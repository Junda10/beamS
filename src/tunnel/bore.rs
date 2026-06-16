use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::{Tunnel, TunnelHandle};
use crate::error::{BeamsError, Result};
use crate::parser::extract_bore_address;

/// Raw TCP tunnel via the `bore` client against the public `bore.pub` server.
pub struct BoreBackend {
    pub binary: PathBuf,
    pub local_port: u16,
}

#[async_trait::async_trait]
impl Tunnel for BoreBackend {
    async fn start(&self) -> Result<TunnelHandle> {
        let port = self.local_port.to_string();
        // bore logs (incl. the assigned address) go to stdout, unlike cloudflared.
        let mut child = Command::new(&self.binary)
            .args(["local", &port, "--to", "bore.pub"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| BeamsError::TunnelStart(e.to_string()))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| BeamsError::TunnelStart("could not read bore output".to_string()))?;
        let mut lines = BufReader::new(stdout).lines();

        let found = tokio::time::timeout(Duration::from_secs(30), async {
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(addr) = extract_bore_address(&line) {
                    return Some(addr);
                }
            }
            None
        })
        .await
        .map_err(|_| BeamsError::UrlTimeout)?;

        match found {
            Some(addr) => {
                // Keep draining stderr so bore doesn't get SIGPIPE on its next
                // log write (same hazard as cloudflared).
                tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
                Ok(TunnelHandle::from_child(addr, child))
            }
            None => Err(BeamsError::TunnelStart(
                "bore exited without providing a public address".to_string(),
            )),
        }
    }
}
