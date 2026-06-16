use std::time::Duration;

use tokio::io::copy_bidirectional;
use tokio::net::TcpStream;
use tokio::sync::broadcast;

use super::{Tunnel, TunnelHandle};
use crate::error::{BeamsError, Result};

const SERVER: &str = "localtunnel.me";

/// Chosen-subdomain HTTPS tunnel implementing the localtunnel (loca.lt) protocol
/// natively: request a subdomain, then maintain a pool of relay connections that
/// proxy incoming requests to the local server.
pub struct LocaltunnelBackend {
    pub subdomain: String,
    pub local_host: String,
    pub local_port: u16,
}

#[async_trait::async_trait]
impl Tunnel for LocaltunnelBackend {
    async fn start(&self) -> Result<TunnelHandle> {
        // 1. Ask the server to assign our subdomain.
        let assign_url = format!("https://{SERVER}/{}", self.subdomain);
        let body = reqwest::Client::new()
            .get(&assign_url)
            .header("User-Agent", "beams")
            .send()
            .await
            .map_err(|e| BeamsError::TunnelStart(e.to_string()))?
            .error_for_status()
            .map_err(|e| BeamsError::TunnelStart(e.to_string()))?
            .bytes()
            .await
            .map_err(|e| BeamsError::TunnelStart(e.to_string()))?;

        let v: serde_json::Value = serde_json::from_slice(&body)
            .map_err(|e| BeamsError::TunnelStart(format!("invalid localtunnel response: {e}")))?;

        let remote_port = v["port"].as_u64().ok_or_else(|| {
            BeamsError::TunnelStart("localtunnel did not return a port".to_string())
        })? as u16;
        let url = v["url"]
            .as_str()
            .ok_or_else(|| BeamsError::TunnelStart("localtunnel did not return a url".to_string()))?
            .to_string();
        let max_conn = v["max_conn_count"].as_u64().unwrap_or(10).clamp(1, 32) as usize;

        // 2. Maintain a pool of relay connections to the server.
        let (shutdown, _) = broadcast::channel::<()>(1);
        let local = format!("{}:{}", self.local_host, self.local_port);
        let remote = format!("{SERVER}:{remote_port}");
        for _ in 0..max_conn {
            let remote = remote.clone();
            let local = local.clone();
            let mut stop = shutdown.subscribe();
            tokio::spawn(async move {
                loop {
                    if stop.try_recv().is_ok() {
                        break;
                    }
                    if relay_once(&remote, &local, &mut stop).await.is_err() {
                        // brief backoff on connection error to avoid a busy loop
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                            _ = stop.recv() => break,
                        }
                    }
                }
            });
        }

        Ok(TunnelHandle::from_task(url, shutdown))
    }
}

/// Open one relay connection (server <-> local) and pipe bytes both ways until
/// either side closes or shutdown fires.
async fn relay_once(
    remote: &str,
    local: &str,
    stop: &mut broadcast::Receiver<()>,
) -> std::io::Result<()> {
    let mut up = TcpStream::connect(remote).await?;
    let mut down = TcpStream::connect(local).await?;
    tokio::select! {
        r = copy_bidirectional(&mut up, &mut down) => r.map(|_| ()),
        _ = stop.recv() => Ok(()),
    }
}
