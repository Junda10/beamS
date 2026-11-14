mod bore;
mod cloudflare;
mod localtunnel;

pub use bore::BoreBackend;
pub use cloudflare::CloudflareBackend;
pub use localtunnel::LocaltunnelBackend;

use crate::error::{BeamsError, Result};

/// Windows' ERROR_VIRUS_INFECTED: the antivirus refused to run the file.
const ERROR_VIRUS_INFECTED: i32 = 225;

/// Turn a failure to launch a tunnel binary into an error the user can act on.
/// Antivirus tools flag tunnelling clients (bore especially) as "potentially
/// unwanted"; the raw OS message doesn't say which file or what to do.
pub(crate) fn spawn_error(binary: &std::path::Path, e: std::io::Error) -> BeamsError {
    if cfg!(windows) && e.raw_os_error() == Some(ERROR_VIRUS_INFECTED) {
        let dir = binary.parent().unwrap_or(binary).display();
        return BeamsError::Blocked(format!(
            "your antivirus blocked {} — tunnelling tools are often flagged as \"potentially unwanted\". Allow it in Windows Security > Virus & threat protection > Protection history, or add an exclusion for {dir}, then run beams again",
            binary.display()
        ));
    }
    BeamsError::TunnelStart(e.to_string())
}

/// A backend that can open a tunnel. Each backend owns its own config.
// async_trait's generated signature trips clippy 1.99's double_must_use.
#[allow(clippy::double_must_use)]
#[async_trait::async_trait]
pub trait Tunnel {
    async fn start(&self) -> Result<TunnelHandle>;
}

// One of these exists per process, so the size difference between the variants
// costs nothing; boxing the `Child` would only add an allocation.
#[allow(clippy::large_enum_variant)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn antivirus_block_gets_its_own_error() {
        let bin = std::path::Path::new("cache/bore.exe");
        let blocked = spawn_error(bin, std::io::Error::from_raw_os_error(ERROR_VIRUS_INFECTED));
        assert_eq!(matches!(blocked, BeamsError::Blocked(_)), cfg!(windows));
        let missing = spawn_error(bin, std::io::Error::from_raw_os_error(2));
        assert!(matches!(missing, BeamsError::TunnelStart(_)));
    }
}
