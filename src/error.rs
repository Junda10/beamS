use thiserror::Error;

#[derive(Error, Debug)]
pub enum BeamsError {
    #[error("unrecognized target \"{0}\" — pass a port (e.g. 3000) or an address (e.g. http://localhost:3000)")]
    InvalidTarget(String),
    #[error("failed to download cloudflared: {0}")]
    Download(String),
    #[error("failed to start tunnel: {0}")]
    TunnelStart(String),
    #[error("timed out waiting for the public URL (cloudflared returned no address within 30s)")]
    UrlTimeout,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, BeamsError>;
