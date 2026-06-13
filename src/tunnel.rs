use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::error::{PharosError, Result};
use crate::parser::extract_public_url;

/// A running tunnel: owns the child process and exposes its public URL.
pub struct TunnelHandle {
    public_url: String,
    child: Child,
}

impl TunnelHandle {
    pub fn public_url(&self) -> &str {
        &self.public_url
    }

    /// Wait for the underlying process to exit.
    pub async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.child.wait().await
    }

    /// Terminate the tunnel.
    pub async fn shutdown(&mut self) -> std::io::Result<()> {
        self.child.kill().await
    }
}

/// A tunnel backend. The MVP ships only `CloudflareBackend`; this trait is the
/// extension point for future backends (bore/TCP, self-hosted, etc.).
#[async_trait::async_trait]
pub trait Tunnel {
    async fn start(&self, target: &str) -> Result<TunnelHandle>;
}

/// Backend that wraps the `cloudflared` quick-tunnel command.
pub struct CloudflareBackend {
    pub binary: PathBuf,
}

#[async_trait::async_trait]
impl Tunnel for CloudflareBackend {
    async fn start(&self, target: &str) -> Result<TunnelHandle> {
        let mut child = Command::new(&self.binary)
            .args(["tunnel", "--no-autoupdate", "--url", target])
            // cloudflared logs (incl. the assigned URL) go to stderr; we don't
            // read stdout, so discard it rather than fill an undrained pipe.
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            // If `start` returns early (timeout / no URL) the handle is dropped;
            // kill_on_drop ensures we don't leak an orphaned cloudflared process.
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| PharosError::TunnelStart(e.to_string()))?;

        // cloudflared prints the assigned URL to stderr.
        let stderr = child.stderr.take().ok_or_else(|| {
            PharosError::TunnelStart("could not read cloudflared output".to_string())
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
        .map_err(|_| PharosError::UrlTimeout)?;

        match found {
            Some(url) => Ok(TunnelHandle {
                public_url: url,
                child,
            }),
            None => Err(PharosError::TunnelStart(
                "cloudflared exited without providing a public URL".to_string(),
            )),
        }
    }
}
