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

/// The reason cloudflared gives before exiting, for a line that carries one.
/// Its fatal startup errors (e.g. "quick tunnel provisioning failed with status
/// 429: ...") are printed as plain text even with `--output json`; JSON lines
/// count only at error/fatal level.
pub fn failure_reason(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let reason = match serde_json::from_str::<serde_json::Value>(line) {
        Ok(v) if matches!(v["level"].as_str(), Some("error" | "fatal")) => {
            let msg = v["message"].as_str().unwrap_or("");
            match v["error"].as_str() {
                Some(err) => format!("{msg}: {err}"),
                None => msg.to_string(),
            }
        }
        Ok(_) => return None,
        Err(_) => line.to_string(),
    };
    Some(reason.chars().take(200).collect())
}

/// `"GET /path?query"` for a cloudflared JSON debug line that logs an incoming
/// request, `None` for anything else. Requests beams makes itself (readiness and
/// liveness probes, tagged by user agent) are skipped. Response lines carry no
/// request id to pair them with, so statuses are not reported.
pub fn request_line(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    v.get("path")?;
    let agent = v["headers"]["User-Agent"][0].as_str().unwrap_or("");
    if agent.starts_with("beams/") {
        return None;
    }
    // message: "GET https://host/path?query HTTP/1.1"
    let mut parts = v["message"].as_str()?.split_whitespace();
    let method = parts.next()?;
    let url = parts.next()?;
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let path = after_scheme.find('/').map_or("/", |i| &after_scheme[i..]);
    Some(format!("{method} {path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_line_from_cloudflared_debug_json() {
        let request = r#"{"connIndex":0,"content-length":0,"event":1,"headers":{"User-Agent":["curl/8.21.0"]},"host":"a-b.trycloudflare.com","level":"debug","message":"GET https://a-b.trycloudflare.com/nope?x=1 HTTP/1.1","originService":"http://127.0.0.1:4190","path":"/nope","time":"2026-09-16T08:17:03Z"}"#;
        assert_eq!(request_line(request).as_deref(), Some("GET /nope?x=1"));

        let response = r#"{"connIndex":0,"content-length":335,"event":1,"level":"debug","message":"404 File not found","originService":"http://127.0.0.1:4190","time":"2026-09-16T08:17:03Z"}"#;
        assert_eq!(request_line(response), None);

        let probe = request.replace("curl/8.21.0", "beams/0.2.3");
        assert_eq!(request_line(&probe), None);

        assert_eq!(request_line("not json"), None);
    }

    #[test]
    fn failure_reason_from_cloudflared_output() {
        let plain = "quick tunnel provisioning failed with status 429: error code: 1015\n";
        assert_eq!(
            failure_reason(plain).as_deref(),
            Some("quick tunnel provisioning failed with status 429: error code: 1015")
        );
        let json_err = r#"{"level":"error","error":"dial tcp: refused","message":"Failed to request quick Tunnel"}"#;
        assert_eq!(
            failure_reason(json_err).as_deref(),
            Some("Failed to request quick Tunnel: dial tcp: refused")
        );
        assert_eq!(
            failure_reason(r#"{"level":"info","message":"Requesting new quick Tunnel"}"#),
            None
        );
        assert_eq!(failure_reason("  \n"), None);
    }

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
