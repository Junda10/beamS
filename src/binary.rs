use std::path::{Path, PathBuf};

use crate::error::{BeamsError, Result};

/// An external CLI that beams downloads and caches on demand.
#[derive(Clone, Copy)]
pub enum Tool {
    Cloudflared,
    Bore,
}

const BORE_VERSION: &str = "v0.6.0";

impl Tool {
    /// File name of the cached executable for this tool.
    pub fn bin_name(self) -> &'static str {
        match self {
            Tool::Cloudflared if cfg!(windows) => "cloudflared.exe",
            Tool::Cloudflared => "cloudflared",
            Tool::Bore if cfg!(windows) => "bore.exe",
            Tool::Bore => "bore",
        }
    }

    /// Name of the entry inside the downloaded archive (without any `.exe`).
    fn archive_entry(self) -> &'static str {
        match self {
            Tool::Cloudflared => "cloudflared",
            Tool::Bore => "bore",
        }
    }

    /// Release asset filename for (os, arch) from `std::env::consts::{OS, ARCH}`.
    pub fn asset(self, os: &str, arch: &str) -> Result<String> {
        let name = match self {
            Tool::Cloudflared => match (os, arch) {
                ("macos", "x86_64") => "cloudflared-darwin-amd64.tgz",
                ("macos", "aarch64") => "cloudflared-darwin-arm64.tgz",
                ("linux", "x86_64") => "cloudflared-linux-amd64",
                ("linux", "aarch64") => "cloudflared-linux-arm64",
                ("windows", "x86_64") => "cloudflared-windows-amd64.exe",
                _ => {
                    return Err(BeamsError::Download(format!(
                        "unsupported platform: {os}/{arch}"
                    )))
                }
            }
            .to_string(),
            Tool::Bore => {
                let triple = match (os, arch) {
                    ("macos", "x86_64") => "x86_64-apple-darwin.tar.gz",
                    ("macos", "aarch64") => "aarch64-apple-darwin.tar.gz",
                    ("linux", "x86_64") => "x86_64-unknown-linux-musl.tar.gz",
                    ("linux", "aarch64") => "aarch64-unknown-linux-musl.tar.gz",
                    ("windows", "x86_64") => "x86_64-pc-windows-msvc.zip",
                    _ => {
                        return Err(BeamsError::Download(format!(
                            "unsupported platform: {os}/{arch}"
                        )))
                    }
                };
                format!("bore-{BORE_VERSION}-{triple}")
            }
        };
        Ok(name)
    }

    /// Download URL for a release asset of this tool.
    pub fn download_url(self, asset: &str) -> String {
        match self {
            Tool::Cloudflared => {
                format!(
                    "https://github.com/cloudflare/cloudflared/releases/latest/download/{asset}"
                )
            }
            Tool::Bore => {
                format!("https://github.com/ekzhang/bore/releases/download/{BORE_VERSION}/{asset}")
            }
        }
    }
}

/// Directory where beams caches downloaded binaries.
pub fn cache_dir() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("dev", "beams", "beams")
        .ok_or_else(|| BeamsError::Download("could not locate a cache directory".to_string()))?;
    Ok(dirs.cache_dir().to_path_buf())
}

/// Full path to the cached executable for `tool`.
pub fn binary_path(tool: Tool) -> Result<PathBuf> {
    Ok(cache_dir()?.join(tool.bin_name()))
}

/// Ensure `tool`'s binary exists in the cache, downloading it on first use.
pub async fn ensure_binary(tool: Tool) -> Result<PathBuf> {
    let path = binary_path(tool)?;
    if path.exists() {
        return Ok(path);
    }
    let asset = tool.asset(std::env::consts::OS, std::env::consts::ARCH)?;
    let url = tool.download_url(&asset);
    std::fs::create_dir_all(cache_dir()?)?;

    let bytes = reqwest::get(&url)
        .await
        .map_err(|e| BeamsError::Download(e.to_string()))?
        .error_for_status()
        .map_err(|e| BeamsError::Download(e.to_string()))?
        .bytes()
        .await
        .map_err(|e| BeamsError::Download(e.to_string()))?;

    // Write to a temp path and atomically rename into place, so an interrupted
    // download can never leave a corrupt binary that a later run treats as valid.
    let tmp = path.with_extension("tmp");
    if asset.ends_with(".zip") {
        extract_from_zip(&bytes, tool.archive_entry(), &tmp)?;
    } else if asset.ends_with(".tgz") || asset.ends_with(".tar.gz") {
        extract_from_tgz(&bytes, tool.archive_entry(), &tmp)?;
    } else {
        std::fs::write(&tmp, &bytes)?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }

    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Extract the entry named `entry` (or `entry.exe`) from a gzip tarball.
fn extract_from_tgz(bytes: &[u8], entry: &str, dest: &Path) -> Result<()> {
    use flate2::read::GzDecoder;
    use tar::Archive;
    let mut archive = Archive::new(GzDecoder::new(bytes));
    for e in archive.entries()? {
        let mut e = e?;
        if entry_matches(e.path()?.file_name(), entry) {
            let mut out = std::fs::File::create(dest)?;
            std::io::copy(&mut e, &mut out)?;
            return Ok(());
        }
    }
    Err(BeamsError::Download(format!(
        "{entry} not found in the downloaded archive"
    )))
}

/// Extract the entry named `entry` (or `entry.exe`) from a zip archive.
fn extract_from_zip(bytes: &[u8], entry: &str, dest: &Path) -> Result<()> {
    use std::io::Cursor;
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| BeamsError::Download(e.to_string()))?;
    for i in 0..zip.len() {
        let mut file = zip
            .by_index(i)
            .map_err(|e| BeamsError::Download(e.to_string()))?;
        let matches = file
            .enclosed_name()
            .as_deref()
            .and_then(|p| p.file_name())
            .map(|f| entry_matches(Some(f), entry))
            .unwrap_or(false);
        if matches {
            let mut out = std::fs::File::create(dest)?;
            std::io::copy(&mut file, &mut out)?;
            return Ok(());
        }
    }
    Err(BeamsError::Download(format!(
        "{entry} not found in the downloaded zip"
    )))
}

/// True if `file_name` is `entry` or `entry.exe`.
fn entry_matches(file_name: Option<&std::ffi::OsStr>, entry: &str) -> bool {
    match file_name.and_then(|n| n.to_str()) {
        Some(n) => n == entry || n == format!("{entry}.exe"),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloudflared_asset_via_tool() {
        assert_eq!(
            Tool::Cloudflared.asset("macos", "aarch64").unwrap(),
            "cloudflared-darwin-arm64.tgz"
        );
        assert_eq!(
            Tool::Cloudflared.asset("linux", "x86_64").unwrap(),
            "cloudflared-linux-amd64"
        );
    }

    #[test]
    fn bore_asset_macos_arm() {
        assert_eq!(
            Tool::Bore.asset("macos", "aarch64").unwrap(),
            "bore-v0.6.0-aarch64-apple-darwin.tar.gz"
        );
    }

    #[test]
    fn bore_asset_linux_x64_is_musl() {
        assert_eq!(
            Tool::Bore.asset("linux", "x86_64").unwrap(),
            "bore-v0.6.0-x86_64-unknown-linux-musl.tar.gz"
        );
    }

    #[test]
    fn bore_asset_windows_is_zip() {
        assert_eq!(
            Tool::Bore.asset("windows", "x86_64").unwrap(),
            "bore-v0.6.0-x86_64-pc-windows-msvc.zip"
        );
    }

    #[test]
    fn unsupported_platform_errors() {
        assert!(matches!(
            Tool::Bore.asset("plan9", "sparc"),
            Err(BeamsError::Download(_))
        ));
    }

    #[test]
    fn bore_download_url_is_pinned() {
        assert_eq!(
            Tool::Bore.download_url("bore-v0.6.0-x86_64-unknown-linux-musl.tar.gz"),
            "https://github.com/ekzhang/bore/releases/download/v0.6.0/bore-v0.6.0-x86_64-unknown-linux-musl.tar.gz"
        );
    }

    #[test]
    fn cloudflared_download_url_is_latest() {
        assert_eq!(
            Tool::Cloudflared.download_url("cloudflared-linux-amd64"),
            "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64"
        );
    }

    #[test]
    fn binary_path_lives_under_cache_dir() {
        let dir = cache_dir().unwrap();
        let path = binary_path(Tool::Cloudflared).unwrap();
        assert!(path.starts_with(&dir));
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name == "cloudflared" || name == "cloudflared.exe");
    }
}
