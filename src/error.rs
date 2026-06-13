use thiserror::Error;

#[derive(Error, Debug)]
pub enum PharosError {
    #[error("无法识别的输入 \"{0}\"，请给出端口号(如 3000)或地址(如 http://localhost:3000)")]
    InvalidTarget(String),
    #[error("下载 cloudflared 失败: {0}")]
    Download(String),
    #[error("启动隧道失败: {0}")]
    TunnelStart(String),
    #[error("等待公网地址超时（30 秒内 cloudflared 未返回地址）")]
    UrlTimeout,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, PharosError>;
