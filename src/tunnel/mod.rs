mod bore;
mod cloudflare;
mod localtunnel;

pub use bore::BoreBackend;
pub use cloudflare::CloudflareBackend;
pub use localtunnel::LocaltunnelBackend;

use crate::error::Result;

/// A backend that can open a tunnel. Each backend owns its own config.
#[async_trait::async_trait]
pub trait Tunnel {
    async fn start(&self) -> Result<TunnelHandle>;
}

enum HandleInner {
    /// External process backends (cloudflared, bore).
    Child(tokio::process::Child),
    /// In-process backend (localtunnel): fire the broadcast to stop.
    Task(tokio::sync::broadcast::Sender<()>),
}

/// A running tunnel: exposes the public address and controls its lifetime.
pub struct TunnelHandle {
    public_url: String,
    inner: HandleInner,
}

impl TunnelHandle {
    pub(crate) fn from_child(public_url: String, child: tokio::process::Child) -> Self {
        Self {
            public_url,
            inner: HandleInner::Child(child),
        }
    }

    pub(crate) fn from_task(
        public_url: String,
        shutdown: tokio::sync::broadcast::Sender<()>,
    ) -> Self {
        Self {
            public_url,
            inner: HandleInner::Task(shutdown),
        }
    }

    pub fn public_url(&self) -> &str {
        &self.public_url
    }

    /// Block until the tunnel ends. Child backends resolve when the process
    /// exits; the in-process backend stays alive until `shutdown` is called, so
    /// this pends forever (the caller relies on Ctrl+C).
    pub async fn wait(&mut self) {
        match &mut self.inner {
            HandleInner::Child(child) => {
                let _ = child.wait().await;
            }
            HandleInner::Task(_) => std::future::pending::<()>().await,
        }
    }

    /// Stop the tunnel.
    pub async fn shutdown(&mut self) {
        match &mut self.inner {
            HandleInner::Child(child) => {
                let _ = child.kill().await;
            }
            HandleInner::Task(tx) => {
                let _ = tx.send(());
            }
        }
    }
}
