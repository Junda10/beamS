use std::path::{Path, PathBuf};

use crate::error::{PharosError, Result};

/// Map (OS, arch) — as given by `std::env::consts::{OS, ARCH}` — to the
/// cloudflared release asset filename.
pub fn cloudflared_asset(os: &str, arch: &str) -> Result<&'static str> {
    Ok(match (os, arch) {
        ("macos", "x86_64") => "cloudflared-darwin-amd64.tgz",
        ("macos", "aarch64") => "cloudflared-darwin-arm64.tgz",
        ("linux", "x86_64") => "cloudflared-linux-amd64",
        ("linux", "aarch64") => "cloudflared-linux-arm64",
        ("windows", "x86_64") => "cloudflared-windows-amd64.exe",
        _ => {
            return Err(PharosError::Download(format!(
                "暂不支持的平台: {os}/{arch}"
            )))
        }
    })
}

/// Build the download URL for a given asset from cloudflared's latest release.
pub fn download_url(asset: &str) -> String {
    format!("https://github.com/cloudflare/cloudflared/releases/latest/download/{asset}")
}

/// Directory where pharos caches the downloaded cloudflared binary.
pub fn cache_dir() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("dev", "pharos", "pharos")
        .ok_or_else(|| PharosError::Download("无法定位缓存目录".to_string()))?;
    Ok(dirs.cache_dir().to_path_buf())
}

/// Full path to the cached cloudflared binary.
pub fn binary_path() -> Result<PathBuf> {
    let name = if cfg!(windows) {
        "cloudflared.exe"
    } else {
        "cloudflared"
    };
    Ok(cache_dir()?.join(name))
}

/// Ensure cloudflared exists in the cache, downloading it on first use.
/// Returns the path to the executable.
pub async fn ensure_binary() -> Result<PathBuf> {
    let path = binary_path()?;
    if path.exists() {
        return Ok(path);
    }
    let asset = cloudflared_asset(std::env::consts::OS, std::env::consts::ARCH)?;
    let url = download_url(asset);
    std::fs::create_dir_all(cache_dir()?)?;

    let bytes = reqwest::get(&url)
        .await
        .map_err(|e| PharosError::Download(e.to_string()))?
        .error_for_status()
        .map_err(|e| PharosError::Download(e.to_string()))?
        .bytes()
        .await
        .map_err(|e| PharosError::Download(e.to_string()))?;

    // Write to a temp path and atomically rename into place, so an interrupted
    // download can never leave a corrupt binary that a later run treats as valid.
    let tmp = path.with_extension("tmp");
    if asset.ends_with(".tgz") {
        extract_cloudflared_from_tgz(&bytes, &tmp)?;
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

/// Extract the `cloudflared` entry from a `.tgz` archive into `dest`.
fn extract_cloudflared_from_tgz(bytes: &[u8], dest: &Path) -> Result<()> {
    use flate2::read::GzDecoder;
    use tar::Archive;
    let mut archive = Archive::new(GzDecoder::new(bytes));
    for entry in archive.entries()? {
        let mut entry = entry?;
        let is_cloudflared = entry
            .path()?
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n == "cloudflared")
            .unwrap_or(false);
        if is_cloudflared {
            let mut out = std::fs::File::create(dest)?;
            std::io::copy(&mut entry, &mut out)?;
            return Ok(());
        }
    }
    Err(PharosError::Download(
        "压缩包中未找到 cloudflared".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_arm_asset() {
        assert_eq!(
            cloudflared_asset("macos", "aarch64").unwrap(),
            "cloudflared-darwin-arm64.tgz"
        );
    }

    #[test]
    fn macos_intel_asset() {
        assert_eq!(
            cloudflared_asset("macos", "x86_64").unwrap(),
            "cloudflared-darwin-amd64.tgz"
        );
    }

    #[test]
    fn linux_amd64_asset() {
        assert_eq!(
            cloudflared_asset("linux", "x86_64").unwrap(),
            "cloudflared-linux-amd64"
        );
    }

    #[test]
    fn windows_asset() {
        assert_eq!(
            cloudflared_asset("windows", "x86_64").unwrap(),
            "cloudflared-windows-amd64.exe"
        );
    }

    #[test]
    fn unsupported_platform_errors() {
        assert!(matches!(
            cloudflared_asset("plan9", "sparc"),
            Err(PharosError::Download(_))
        ));
    }

    #[test]
    fn download_url_points_at_latest_release() {
        assert_eq!(
            download_url("cloudflared-linux-amd64"),
            "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64"
        );
    }

    #[test]
    fn binary_path_lives_under_cache_dir_and_is_named_cloudflared() {
        let dir = cache_dir().unwrap();
        let path = binary_path().unwrap();
        assert!(path.starts_with(&dir));
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name == "cloudflared" || name == "cloudflared.exe");
    }
}
