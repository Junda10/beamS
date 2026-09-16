use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{ChildStderr, Command};

use super::{Tunnel, TunnelHandle};
use crate::error::{BeamsError, Result};
use crate::output;
use crate::parser::{extract_public_url, request_line};

/// Random-URL HTTPS tunnel via `cloudflared` quick tunnel.
pub struct CloudflareBackend {
    pub binary: PathBuf,
    /// Forwarding target, e.g. "http://localhost:3000".
    pub target: String,
    /// Edge transport to pin: "quic", "http2", or "auto". `None` leaves
    /// cloudflared on its own default (quic, falling back to http2).
    pub protocol: Option<String>,
}

impl CloudflareBackend {
    /// The `host:port` to send as the Host header to the local server.
    fn host_header(&self) -> &str {
        self.target
            .strip_prefix("http://")
            .or_else(|| self.target.strip_prefix("https://"))
            .unwrap_or(&self.target)
            .split('/')
            .next()
            .unwrap_or(&self.target)
    }
}

#[async_trait::async_trait]
impl Tunnel for CloudflareBackend {
    async fn start(&self) -> Result<TunnelHandle> {
        // Rewrite the Host header to the local host:port. Dev servers (Vite,
        // webpack-dev-server, …) reject requests whose Host is the public tunnel
        // domain; sending `localhost:PORT` makes them work out of the box.
        // Debug level is the only one where cloudflared logs each proxied request;
        // JSON makes those lines parseable. It all stays in our pipe.
        let mut args: Vec<&str> = vec![
            "tunnel",
            "--no-autoupdate",
            "--loglevel",
            "debug",
            "--output",
            "json",
            "--http-host-header",
            self.host_header(),
            "--url",
            self.target.as_str(),
        ];
        // cloudflared defaults to QUIC over UDP/7844. Networks that throttle or
        // drop UDP (campus, corporate, some ISPs) make that path slow or flaky,
        // and cloudflared's own switch to http2 costs seconds each time; pinning
        // the transport up front skips that.
        if let Some(protocol) = &self.protocol {
            args.push("--protocol");
            args.push(protocol.as_str());
        }

        let mut child = Command::new(&self.binary)
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| BeamsError::TunnelStart(e.to_string()))?;

        let stderr = child.stderr.take().ok_or_else(|| {
            BeamsError::TunnelStart("could not read cloudflared output".to_string())
        })?;
        let mut stderr = BufReader::new(stderr);
        let mut buf = Vec::new();

        let found = tokio::time::timeout(Duration::from_secs(30), async {
            while let Some(line) = next_line(&mut stderr, &mut buf).await {
                if let Some(url) = extract_public_url(&line) {
                    return Some(url);
                }
            }
            None
        })
        .await
        .map_err(|_| BeamsError::UrlTimeout)?;

        match found {
            Some(url) => {
                // Keep draining stderr for the tunnel's lifetime; if we stop
                // reading, the pipe closes and cloudflared dies with SIGPIPE on
                // its next log write, killing the tunnel right after it comes up.
                tokio::spawn(async move {
                    while let Some(line) = next_line(&mut stderr, &mut buf).await {
                        if let Some(request) = request_line(&line) {
                            output::print_request(&request);
                        }
                    }
                });
                Ok(TunnelHandle::from_child(url, child))
            }
            None => Err(BeamsError::TunnelStart(
                "cloudflared exited without providing a public URL".to_string(),
            )),
        }
    }
}

/// Read one line of cloudflared output. Byte-based on purpose: `lines()` errors
/// on invalid UTF-8, and a drain loop that stops reading lets the pipe fill or
/// close and takes the tunnel down with it. `None` only at EOF or a read error.
async fn next_line(reader: &mut BufReader<ChildStderr>, buf: &mut Vec<u8>) -> Option<String> {
    buf.clear();
    match reader.read_until(b'\n', buf).await {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(String::from_utf8_lossy(buf).into_owned()),
    }
}
