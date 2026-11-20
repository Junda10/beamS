//! A once-a-day "a newer beams is out" hint.
//!
//! The hint comes from what an earlier run cached, so checking never slows
//! startup or blocks on the network. When that answer is a day old, a refresh
//! runs in the background and the next run sees it. Any failure (offline,
//! registry down, unreadable cache) just means no hint.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// npm is where people install and update beams from, so ask it.
const LATEST_URL: &str = "https://registry.npmjs.org/beams-cli/latest";
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const CACHE_FILE: &str = "latest-version";

/// The newer version to mention, if an earlier check found one. Also starts a
/// background refresh when the cached answer is stale. Needs a Tokio runtime.
/// Set `BEAMS_NO_UPDATE_CHECK` to turn it off; it is also off in CI.
pub fn check() -> Option<String> {
    if std::env::var_os("BEAMS_NO_UPDATE_CHECK").is_some() || std::env::var_os("CI").is_some() {
        return None;
    }
    let path = crate::binary::cache_dir().ok()?.join(CACHE_FILE);
    let cached = read_cache(&path);
    let now = now_secs();
    // A check stamped in the future (the clock was wrong) counts as stale.
    let fresh = cached
        .as_ref()
        .is_some_and(|(at, _)| *at <= now && now - at < CHECK_EVERY.as_secs());
    if !fresh {
        tokio::spawn(refresh(path, cached.as_ref().map(|(_, v)| v.clone())));
    }
    newer(env!("CARGO_PKG_VERSION"), &cached?.1)
}

/// `latest` as `x.y.z` when it is a newer stable release than `current`.
/// Pre-releases are never suggested.
pub fn newer(current: &str, latest: &str) -> Option<String> {
    let (cur, new) = (parse(current)?, parse(latest)?);
    (new > cur).then(|| format!("{}.{}.{}", new.0, new.1, new.2))
}

fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.trim().trim_start_matches('v').split('.');
    let v = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(v)
}

/// The cache holds `<unix seconds of the check> <version>`.
fn read_cache(path: &Path) -> Option<(u64, String)> {
    let text = std::fs::read_to_string(path).ok()?;
    let (at, version) = text.trim().split_once(' ')?;
    Some((at.parse().ok()?, version.to_string()))
}

/// Record today's check. A failed fetch keeps the version we already knew, so
/// being offline costs one request a day rather than one per launch.
async fn refresh(path: PathBuf, previous: Option<String>) {
    let Some(version) = fetch_latest().await.or(previous) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = tokio::fs::create_dir_all(dir).await;
    }
    let _ = tokio::fs::write(&path, format!("{} {version}\n", now_secs())).await;
}

async fn fetch_latest() -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .user_agent(concat!("beams/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let body = client
        .get(LATEST_URL)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .await
        .ok()?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    let version = json.get("version")?.as_str()?;
    parse(version).map(|_| version.to_string())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_only_for_a_higher_stable_version() {
        assert_eq!(newer("0.2.8", "0.2.9").as_deref(), Some("0.2.9"));
        assert_eq!(newer("0.2.8", "0.10.0").as_deref(), Some("0.10.0"));
        assert_eq!(newer("0.2.8", "v1.0.0").as_deref(), Some("1.0.0"));
        assert_eq!(newer("0.2.8", "0.2.8"), None);
        assert_eq!(newer("0.2.8", "0.2.7"), None);
    }

    #[test]
    fn newer_ignores_prereleases_and_junk() {
        assert_eq!(newer("0.2.8", "0.3.0-beta.1"), None);
        assert_eq!(newer("0.2.8", "0.3"), None);
        assert_eq!(newer("0.2.8", "0.3.0.1"), None);
        assert_eq!(newer("0.2.8", "\x1b[31m9.9.9"), None);
        assert_eq!(newer("0.2.8", ""), None);
    }

    #[test]
    fn cache_round_trips_and_rejects_garbage() {
        let dir = std::env::temp_dir().join(format!("beams-update-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CACHE_FILE);

        std::fs::write(&path, "1700000000 0.3.0\n").unwrap();
        assert_eq!(read_cache(&path), Some((1_700_000_000, "0.3.0".into())));

        std::fs::write(&path, "not a cache").unwrap();
        assert_eq!(read_cache(&path), None);

        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(read_cache(&path), None);
    }
}
