use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::{Tunnel, TunnelHandle};
use crate::error::{BeamsError, Result};
use crate::parser::extract_public_url;

/// Random-URL HTTPS tunnel via `cloudflared` quick tunnel.
pub struct CloudflareBackend {
    pub binary: PathBuf,
    /// Forwarding target, e.g. "http://localhost:3000".
    pub target: String,
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
        let mut child = Command::new(&self.binary)
            .args([
                "tunnel",
                "--no-autoupdate",
                "--http-host-header",
                self.host_header(),
                "--url",
                &self.target,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| BeamsError::TunnelStart(e.to_string()))?;

        let stderr = child.stderr.take().ok_or_else(|| {
            BeamsError::TunnelStart("could not read cloudflared output".to_string())
        })?;
        let mut lines = BufReader::new(stderr).lines();

        let found = tokio::time::timeout(Duration::from_secs(30), async {
            while let Ok(Some(line)) = lines.next_line().await {
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
                tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
                Ok(TunnelHandle::from_child(url, child))
            }
            None => Err(BeamsError::TunnelStart(
                "cloudflared exited without providing a public URL".to_string(),
            )),
        }
    }
}
