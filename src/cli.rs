use crate::error::{BeamsError, Result};

/// Normalize user input into a URL `cloudflared` can forward to.
///
/// - `"3000"`              -> `"http://localhost:3000"`
/// - `"http://..."`        -> unchanged
/// - `"localhost:3000"`    -> `"http://localhost:3000"`
pub fn parse_target(input: &str) -> Result<String> {
    let input = input.trim();
    if input.is_empty() {
        return Err(BeamsError::InvalidTarget(input.to_string()));
    }
    if input.starts_with("http://") || input.starts_with("https://") {
        return Ok(input.to_string());
    }
    // bare port: all digits, 1..=65535
    if input.chars().all(|c| c.is_ascii_digit()) {
        match input.parse::<u32>() {
            Ok(p) if (1..=65535).contains(&p) => return Ok(format!("http://localhost:{p}")),
            _ => return Err(BeamsError::InvalidTarget(input.to_string())),
        }
    }
    // host:port without scheme -> assume http
    if let Some((host, port)) = input.rsplit_once(':') {
        if !host.is_empty() && !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) {
            return Ok(format!("http://{input}"));
        }
    }
    Err(BeamsError::InvalidTarget(input.to_string()))
}

/// Extract `(host, port)` from a port / `host:port` / URL input.
/// Used by the TCP (bore) and subdomain (localtunnel) backends, which need a
/// local host and port rather than a forwarding URL.
pub fn parse_host_port(input: &str) -> Result<(String, u16)> {
    let s = input.trim();
    let s = s
        .strip_prefix("http://")
        .or_else(|| s.strip_prefix("https://"))
        .unwrap_or(s);
    let authority = s.split('/').next().unwrap_or(s);
    if authority.is_empty() {
        return Err(BeamsError::InvalidTarget(input.to_string()));
    }
    if let Some((host, port)) = authority.rsplit_once(':') {
        let port: u16 = port
            .parse()
            .map_err(|_| BeamsError::InvalidTarget(input.to_string()))?;
        if host.is_empty() || port == 0 {
            return Err(BeamsError::InvalidTarget(input.to_string()));
        }
        return Ok((host.to_string(), port));
    }
    let port: u16 = authority
        .parse()
        .map_err(|_| BeamsError::InvalidTarget(input.to_string()))?;
    if port == 0 {
        return Err(BeamsError::InvalidTarget(input.to_string()));
    }
    Ok(("localhost".to_string(), port))
}

/// Turn an answer to the "which port?" prompt into a target: empty picks the
/// first entry, `1..=n` picks from the list, anything else is taken as a
/// port/address typed by hand. `None` means an out-of-range list number.
pub fn pick_target(answer: &str, ports: &[u16]) -> Option<String> {
    let answer = answer.trim();
    if answer.is_empty() {
        return ports.first().map(u16::to_string);
    }
    match answer.parse::<usize>() {
        Ok(n) if n <= ports.len() => n.checked_sub(1).map(|i| ports[i].to_string()),
        _ => Some(answer.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_target_from_prompt_answer() {
        let ports = [3000, 5173];
        assert_eq!(pick_target("", &ports).as_deref(), Some("3000"));
        assert_eq!(
            pick_target(
                "2
", &ports
            )
            .as_deref(),
            Some("5173")
        );
        assert_eq!(pick_target("0", &ports), None);
        assert_eq!(pick_target("8081", &ports).as_deref(), Some("8081"));
        assert_eq!(
            pick_target("localhost:9000", &ports).as_deref(),
            Some("localhost:9000")
        );
    }

    #[test]
    fn host_port_from_bare_port() {
        assert_eq!(
            parse_host_port("3000").unwrap(),
            ("localhost".to_string(), 3000)
        );
    }

    #[test]
    fn host_port_from_host_colon_port() {
        assert_eq!(
            parse_host_port("127.0.0.1:5432").unwrap(),
            ("127.0.0.1".to_string(), 5432)
        );
    }

    #[test]
    fn host_port_from_url() {
        assert_eq!(
            parse_host_port("http://localhost:8080").unwrap(),
            ("localhost".to_string(), 8080)
        );
    }

    #[test]
    fn host_port_rejects_garbage() {
        assert!(matches!(
            parse_host_port("nope"),
            Err(BeamsError::InvalidTarget(_))
        ));
    }

    #[test]
    fn bare_port_becomes_localhost_url() {
        assert_eq!(parse_target("3000").unwrap(), "http://localhost:3000");
    }

    #[test]
    fn full_http_url_is_unchanged() {
        assert_eq!(
            parse_target("http://localhost:8080").unwrap(),
            "http://localhost:8080"
        );
    }

    #[test]
    fn full_https_url_is_unchanged() {
        assert_eq!(
            parse_target("https://127.0.0.1:8080").unwrap(),
            "https://127.0.0.1:8080"
        );
    }

    #[test]
    fn host_port_without_scheme_gets_http() {
        assert_eq!(
            parse_target("localhost:3000").unwrap(),
            "http://localhost:3000"
        );
    }

    #[test]
    fn port_zero_is_invalid() {
        assert!(matches!(
            parse_target("0"),
            Err(BeamsError::InvalidTarget(_))
        ));
    }

    #[test]
    fn garbage_is_invalid() {
        assert!(matches!(
            parse_target("not a target"),
            Err(BeamsError::InvalidTarget(_))
        ));
    }
}
