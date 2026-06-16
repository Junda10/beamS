/// Pull a `https://<sub>.trycloudflare.com` URL out of a cloudflared log line.
/// Returns `None` if the line contains no such URL.
pub fn extract_public_url(line: &str) -> Option<String> {
    let start = line.find("https://")?;
    let rest = &line[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '|')
        .unwrap_or(rest.len());
    let url = &rest[..end];
    if url.ends_with(".trycloudflare.com") {
        Some(url.to_string())
    } else {
        None
    }
}

/// Pull the public bore address (`bore.pub:<port>`) out of a bore log line.
/// Handles both the "listening at bore.pub:PORT" line and a "remote_port=PORT"
/// field, returning a normalized `bore.pub:<port>` string.
pub fn extract_bore_address(line: &str) -> Option<String> {
    for marker in ["bore.pub:", "remote_port="] {
        if let Some(idx) = line.find(marker) {
            let rest = &line[idx + marker.len()..];
            let port: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !port.is_empty() {
                return Some(format!("bore.pub:{port}"));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_bore_address() {
        let line = "2024-01-01 INFO bore_cli::client: listening at bore.pub:41234";
        assert_eq!(
            extract_bore_address(line).as_deref(),
            Some("bore.pub:41234")
        );
    }

    #[test]
    fn extracts_bore_remote_port_field() {
        let line = "INFO connected to server remote_port=41234";
        assert_eq!(
            extract_bore_address(line).as_deref(),
            Some("bore.pub:41234")
        );
    }

    #[test]
    fn bore_ignores_unrelated() {
        assert_eq!(extract_bore_address("INFO starting client"), None);
    }

    #[test]
    fn extracts_url_from_boxed_log_line() {
        let line = "2024-01-01T00:00:00Z INF |  https://happy-cat-42.trycloudflare.com   |";
        assert_eq!(
            extract_public_url(line).as_deref(),
            Some("https://happy-cat-42.trycloudflare.com")
        );
    }

    #[test]
    fn extracts_plain_url() {
        let line = "https://blue-tree-7.trycloudflare.com";
        assert_eq!(
            extract_public_url(line).as_deref(),
            Some("https://blue-tree-7.trycloudflare.com")
        );
    }

    #[test]
    fn ignores_non_trycloudflare_urls() {
        let line = "INF connecting to https://api.cloudflare.com/foo";
        assert_eq!(extract_public_url(line), None);
    }

    #[test]
    fn ignores_lines_without_url() {
        assert_eq!(extract_public_url("INF starting tunnel"), None);
    }
}
