use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use colored::Colorize;
use tokio::task::JoinHandle;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const GITHUB_API_URL: &str =
    "https://api.github.com/repos/geckse/markdown-vdb/releases/latest";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Spawn a non-blocking background update check.
/// Returns a `JoinHandle` that resolves to `Some(message)` if a newer version
/// is available, or `None` otherwise. All errors are silently swallowed.
pub fn spawn_check() -> JoinHandle<Option<String>> {
    tokio::spawn(async { check_for_update().await.unwrap_or(None) })
}

fn cache_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".mdvdb").join("last-update-check"))
}

async fn check_for_update(
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    if std::env::var("MDVDB_NO_UPDATE_CHECK").is_ok() {
        return Ok(None);
    }

    let cache = match cache_path() {
        Some(p) => p,
        None => return Ok(None),
    };

    // Check cache freshness
    if let Ok(contents) = tokio::fs::read_to_string(&cache).await {
        if let Some((ts_str, cached_version)) = contents.split_once('\n') {
            if let Ok(ts) = ts_str.parse::<u64>() {
                let cached_time = UNIX_EPOCH + Duration::from_secs(ts);
                if SystemTime::now()
                    .duration_since(cached_time)
                    .unwrap_or(Duration::MAX)
                    < CHECK_INTERVAL
                {
                    return Ok(format_update_message(cached_version.trim()));
                }
            }
        }
    }

    // Fetch latest version from GitHub
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()?;

    let resp: serde_json::Value = client
        .get(GITHUB_API_URL)
        .header("User-Agent", format!("mdvdb/{}", CURRENT_VERSION))
        .send()
        .await?
        .json()
        .await?;

    let tag = resp["tag_name"]
        .as_str()
        .ok_or("missing tag_name in GitHub API response")?;

    let latest = tag.strip_prefix('v').unwrap_or(tag);

    // Write cache (best effort)
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if let Some(parent) = cache.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let _ = tokio::fs::write(&cache, format!("{}\n{}", now, latest)).await;

    Ok(format_update_message(latest))
}

fn format_update_message(latest: &str) -> Option<String> {
    let current = semver::Version::parse(CURRENT_VERSION).ok()?;
    let latest_ver = semver::Version::parse(latest).ok()?;

    if latest_ver > current {
        Some(format!(
            "\n{} {} → {} (run `curl -fsSL https://raw.githubusercontent.com/geckse/markdown-vdb/main/install.sh | sh` to update)",
            "Update available:".yellow(),
            CURRENT_VERSION,
            latest.bold(),
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_update_when_current() {
        assert!(format_update_message(CURRENT_VERSION).is_none());
    }

    #[test]
    fn update_available_for_newer_version() {
        let msg = format_update_message("99.0.0");
        assert!(msg.is_some());
        let text = msg.unwrap();
        assert!(text.contains("99.0.0"));
        assert!(text.contains(CURRENT_VERSION));
    }

    #[test]
    fn no_update_for_older_version() {
        assert!(format_update_message("0.0.1").is_none());
    }

    #[test]
    fn no_update_for_invalid_semver() {
        assert!(format_update_message("not-a-version").is_none());
    }

    #[test]
    fn no_update_for_empty_string() {
        assert!(format_update_message("").is_none());
    }

    #[test]
    fn cache_path_exists() {
        // Should return Some on any system with a home directory
        let path = cache_path();
        assert!(path.is_some());
        let p = path.unwrap();
        assert!(p.ends_with("last-update-check"));
        assert!(p.to_string_lossy().contains(".mdvdb"));
    }

    #[tokio::test]
    async fn check_respects_opt_out() {
        std::env::set_var("MDVDB_NO_UPDATE_CHECK", "1");
        let result = check_for_update().await;
        std::env::remove_var("MDVDB_NO_UPDATE_CHECK");
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }
}
