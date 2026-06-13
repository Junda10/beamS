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

#[cfg(test)]
mod tests {
    use super::*;

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
