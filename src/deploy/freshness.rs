//! Throttled "is there a newer berth on GitHub?" check.
//!
//! We deliberately do NOT auto-update the local binary — install method
//! (cargo build, curl, distro pkg, etc.) is the user's choice. We just
//! emit a one-line warning when local is behind, throttled to once per
//! 24 hours so it doesn't introduce a network round-trip to every CLI
//! invocation.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const STATE_FILE_NAME: &str = "last_version_check";

fn state_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("BERTH_CACHE_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let base = dirs::cache_dir().context("locating cache directory")?;
    Ok(base.join("berth"))
}

fn state_file() -> Result<PathBuf> {
    Ok(state_dir()?.join(STATE_FILE_NAME))
}

fn should_check_now() -> bool {
    let Ok(path) = state_file() else {
        return true;
    };
    let Ok(meta) = std::fs::metadata(&path) else {
        return true;
    };
    let Ok(mtime) = meta.modified() else {
        return true;
    };
    let Ok(elapsed) = SystemTime::now().duration_since(mtime) else {
        return true;
    };
    elapsed >= CHECK_INTERVAL
}

fn touch_state_file() {
    if let Ok(dir) = state_dir() {
        let _ = std::fs::create_dir_all(&dir);
    }
    if let Ok(path) = state_file() {
        let _ = std::fs::write(&path, b"");
    }
}

/// Query the GitHub Releases API for the latest tag of this repo.
async fn fetch_latest_tag() -> Result<String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/releases/latest",
        super::fetch::REPO_OWNER,
        super::fetch::REPO_NAME
    );
    let client = reqwest::Client::builder()
        .user_agent(concat!("berth/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(5))
        .build()?;
    let body = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    // Parse just the "tag_name" field without pulling in a JSON crate
    // beyond what we already have.
    let v: serde_json::Value = serde_json::from_str(&body)?;
    let tag = v
        .get("tag_name")
        .and_then(|t| t.as_str())
        .context("GitHub release response missing tag_name")?
        .to_string();
    Ok(tag)
}

/// Best-effort, throttled freshness check. Returns the latest release
/// tag if local is strictly older. Network failures and parse errors
/// silently return `Ok(None)` — we never block real work on the check.
pub async fn check() -> Result<Option<String>> {
    if !should_check_now() {
        return Ok(None);
    }
    let local_version = env!("CARGO_PKG_VERSION");
    let latest_tag = match fetch_latest_tag().await {
        Ok(t) => t,
        Err(_) => {
            // Network down, GitHub rate-limit, whatever — don't pester
            // the user. Touch the state file so we don't retry every
            // command.
            touch_state_file();
            return Ok(None);
        }
    };
    touch_state_file();
    let latest_clean = latest_tag.trim_start_matches('v');
    let local_parsed = semver::Version::parse(local_version);
    let latest_parsed = semver::Version::parse(latest_clean);
    let stale = match (local_parsed, latest_parsed) {
        (Ok(local), Ok(latest)) => local < latest,
        _ => latest_clean != local_version,
    };
    Ok(if stale { Some(latest_tag) } else { None })
}

/// Emit a one-line warning on stderr if the local binary is behind.
/// Suggests the matching artifact name for self-update.
pub async fn warn_if_stale() {
    let Ok(Some(latest)) = check().await else {
        return;
    };
    let local = env!("CARGO_PKG_VERSION");
    let install_hint = match super::local::local_target_triple() {
        Some(target) => format!("download `berth-{target}.tar.gz` from the {latest} release"),
        None => format!("rebuild from source at tag {latest}"),
    };
    eprintln!("berth: local v{local} is behind {latest}; consider updating: {install_hint}");
}
