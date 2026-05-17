//! Fetch a pre-built berth binary from this project's GitHub releases.
//!
//! Asset layout (per release tag `v<x.y.z>`):
//!   - `berth-<target>.tar.gz`       — gzipped tar containing the `berth` ELF
//!   - `berth-<target>.tar.gz.sha256` — `<hex>  berth-<target>.tar.gz`
//!
//! The fetcher downloads both, verifies the SHA256, extracts the binary
//! into a per-target cache directory under `$XDG_CACHE_HOME/berth/binaries/`,
//! and returns the path. Re-runs are no-ops when the cached binary already
//! matches.

use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::fs;

pub(super) const REPO_OWNER: &str = "morgaesis";
pub(super) const REPO_NAME: &str = "berth";

/// Override for tests / mirrors: when set, used as the base URL instead of
/// `https://github.com/<owner>/<repo>/releases/download`.
const BASE_URL_ENV: &str = "BERTH_RELEASE_BASE_URL";

/// Cache directory for downloaded binaries.
pub fn cache_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("BERTH_CACHE_DIR") {
        return Ok(PathBuf::from(dir).join("binaries"));
    }
    let base = dirs::cache_dir().context("locating cache directory")?;
    Ok(base.join("berth").join("binaries"))
}

/// Max download size for any release asset (binary or sidecar). Defends
/// against an OOM-by-large-archive scenario before SHA verification can
/// run. 200 MiB is well above any plausible berth binary + tar overhead
/// and well below "would make your laptop swap".
const MAX_DOWNLOAD_BYTES: usize = 200 * 1024 * 1024;

fn base_url() -> Result<String> {
    if let Ok(url) = std::env::var(BASE_URL_ENV) {
        let url = url.trim_end_matches('/').to_string();
        if !url.starts_with("https://") {
            bail!(
                "{BASE_URL_ENV}={url:?} must be an https:// URL; refusing to fetch over plaintext"
            );
        }
        return Ok(url);
    }
    Ok(format!(
        "https://github.com/{REPO_OWNER}/{REPO_NAME}/releases/download"
    ))
}

/// Asset filename for a given target.
pub fn asset_name(target: &str) -> String {
    format!("berth-{target}.tar.gz")
}

/// Fetch the binary for `tag` + `target`. Returns the local path of the
/// extracted `berth` binary, ready to be `scp`'d.
#[tracing::instrument(level = "debug", fields(tag = %tag, target = %target))]
pub async fn fetch_binary(tag: &str, target: &str) -> Result<PathBuf> {
    let cache = cache_dir()?;
    fs::create_dir_all(&cache)
        .await
        .with_context(|| format!("creating cache dir {}", cache.display()))?;
    tracing::debug!(cache = %cache.display(), "cache dir ready");

    let final_path = cache.join(format!("berth-{}-{target}", tag.trim_start_matches('v')));
    if final_path.exists() {
        tracing::info!(path = %final_path.display(), "cache hit; skipping download");
        return Ok(final_path);
    }
    tracing::debug!(path = %final_path.display(), "cache miss; will download");

    let asset = asset_name(target);
    let tag_path = tag.trim_start_matches('v');
    // Validate `tag` here as the second line of defense — the CLI also
    // validates, but `ensure_deployed` is callable from `enter.rs` with
    // an internally-constructed `v<CARGO_PKG_VERSION>` tag and we want
    // any future internal caller to be safe too.
    crate::validate_release_tag(tag_path)?;
    let base = base_url()?;
    let bin_url = format!("{base}/v{tag_path}/{asset}");
    let sha_url = format!("{bin_url}.sha256");

    tracing::info!(bin_url = %bin_url, sha_url = %sha_url, "fetching release assets");
    let sha_text = http_get_text(&sha_url)
        .await
        .with_context(|| format!("fetching {sha_url}"))?;
    tracing::debug!(
        sha_lines = sha_text.lines().count(),
        "fetched sha256 sidecar"
    );
    let archive_bytes = http_get_bytes_with_progress(&bin_url, Some("downloading"))
        .await
        .with_context(|| format!("fetching {bin_url}"))?;
    tracing::debug!(bytes = archive_bytes.len(), "fetched archive");

    let expected = parse_sha256_sidecar(&sha_text, &asset)
        .with_context(|| format!("parsing sha256 sidecar for {asset}"))?;
    let actual = sha256_hex(&archive_bytes);
    tracing::debug!(expected = %expected, actual = %actual, "sha256 comparison");
    if actual != expected {
        bail!("sha256 mismatch for {asset}: expected {expected}, got {actual}");
    }
    tracing::debug!("sha256 verified");

    let extracted = extract_berth_from_targz(&archive_bytes)
        .with_context(|| format!("extracting berth binary from {asset}"))?;
    tracing::debug!(
        bytes = extracted.len(),
        "extracted berth binary from archive"
    );

    // Atomic place via a sibling tempfile to avoid half-written caches.
    let tmp = final_path.with_extension("partial");
    fs::write(&tmp, &extracted)
        .await
        .with_context(|| format!("writing {}", tmp.display()))?;
    set_executable(&tmp).await?;
    fs::rename(&tmp, &final_path)
        .await
        .with_context(|| format!("renaming to {}", final_path.display()))?;
    Ok(final_path)
}

async fn http_get_bytes(url: &str) -> Result<Vec<u8>> {
    http_get_bytes_with_progress(url, None).await
}

async fn http_get_bytes_with_progress(url: &str, label: Option<&str>) -> Result<Vec<u8>> {
    let resp = http_client()?
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()?;
    let total = resp.content_length();
    let bar = label.map(|name| {
        let pb = match total {
            Some(len) => {
                let pb = ProgressBar::new(len);
                pb.set_style(
                    ProgressStyle::with_template(
                        "  {msg:>20.cyan} [{bar:32.green/white}] {bytes:>10}/{total_bytes:<10} {bytes_per_sec}",
                    )
                    .unwrap()
                    .progress_chars("=> "),
                );
                pb
            }
            None => {
                let pb = ProgressBar::new_spinner();
                pb.enable_steady_tick(Duration::from_millis(80));
                pb.set_style(
                    ProgressStyle::with_template("  {msg:>20.cyan} {spinner} {bytes}")
                        .unwrap()
                        .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ "),
                );
                pb
            }
        };
        pb.set_message(name.to_string());
        pb
    });
    let mut stream = resp.bytes_stream();
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if out.len().saturating_add(chunk.len()) > MAX_DOWNLOAD_BYTES {
            if let Some(b) = &bar {
                b.abandon_with_message(format!("ABORT >{}MiB", MAX_DOWNLOAD_BYTES / (1024 * 1024)));
            }
            bail!(
                "download from {url} exceeded {} MiB cap; aborting",
                MAX_DOWNLOAD_BYTES / (1024 * 1024)
            );
        }
        out.extend_from_slice(&chunk);
        if let Some(b) = &bar {
            b.set_position(out.len() as u64);
        }
    }
    if let Some(b) = bar {
        b.finish_and_clear();
    }
    Ok(out)
}

async fn http_get_text(url: &str) -> Result<String> {
    let bytes = http_get_bytes(url).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("berth/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()?)
}

/// Parse a `sha256sum`-style sidecar: `<64-hex>  <filename>` (two spaces).
/// Returns the hex for the asset whose filename matches.
fn parse_sha256_sidecar(text: &str, asset: &str) -> Result<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let hex = parts.next().context("sha256 sidecar line missing hex")?;
        let name = parts.next();
        if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("invalid hex in sha256 sidecar: {hex}");
        }
        if name.is_none() || name == Some(asset) || name == Some(&format!("./{asset}")) {
            return Ok(hex.to_ascii_lowercase());
        }
    }
    bail!("no entry for {asset} in sha256 sidecar")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Tiny gzip+tar reader that pulls a single file named `berth` from the
/// archive. We don't want to drag in the `tar`/`flate2` ecosystems just
/// for one-file extraction — we shell out to system `tar` instead, which
/// is universally available on every platform where berth itself can run.
fn extract_berth_from_targz(archive: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    tracing::debug!(archive_bytes = archive.len(), "spawning tar -xzOf -");
    let mut child = Command::new("tar")
        .arg("-xzOf")
        .arg("-")
        .arg("berth")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning system `tar` for archive extraction")?;

    // Write the archive to tar's stdin on a separate thread so that we
    // can concurrently drain stdout/stderr via `wait_with_output`. The
    // previous serial write-then-wait pattern deadlocks on archives
    // larger than the OS pipe buffer (typically 64 KiB) when tar starts
    // emitting `berth` to stdout faster than this side can drain it.
    let archive = archive.to_vec();
    let mut stdin = child.stdin.take().context("tar stdin missing")?;
    let writer = std::thread::spawn(move || {
        stdin
            .write_all(&archive)
            .context("piping archive bytes to tar")
    });

    let out = child
        .wait_with_output()
        .context("waiting for tar to finish")?;
    writer.join().expect("tar writer thread panicked")?;
    tracing::debug!(stdout_bytes = out.stdout.len(), "tar finished");
    if !out.status.success() {
        bail!(
            "tar -xzOf failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    if out.stdout.is_empty() {
        bail!("tar produced no output; archive may not contain a `berth` file");
    }
    if out.stdout.len() > MAX_DOWNLOAD_BYTES {
        bail!(
            "extracted berth binary exceeded {} MiB cap; archive likely malicious",
            MAX_DOWNLOAD_BYTES / (1024 * 1024)
        );
    }
    Ok(out.stdout)
}

#[cfg(unix)]
async fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path).await?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).await?;
    Ok(())
}

#[cfg(not(unix))]
async fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_matches_known_value() {
        // sha256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        assert_eq!(
            sha256_hex(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn parse_sha256_sidecar_two_space_form() {
        let text = "deadbeef00000000000000000000000000000000000000000000000000000000  berth-x86_64-unknown-linux-musl.tar.gz\n";
        let got = parse_sha256_sidecar(text, "berth-x86_64-unknown-linux-musl.tar.gz").unwrap();
        assert_eq!(
            got,
            "deadbeef00000000000000000000000000000000000000000000000000000000"
        );
    }

    #[test]
    fn parse_sha256_sidecar_rejects_bad_hex() {
        let text = "zz  berth.tar.gz\n";
        assert!(parse_sha256_sidecar(text, "berth.tar.gz").is_err());
    }

    #[test]
    fn parse_sha256_sidecar_rejects_missing_entry() {
        let text =
            "deadbeef00000000000000000000000000000000000000000000000000000000  other.tar.gz\n";
        assert!(parse_sha256_sidecar(text, "berth-x86_64-unknown-linux-musl.tar.gz").is_err());
    }

    #[test]
    fn asset_name_format() {
        assert_eq!(
            asset_name("x86_64-unknown-linux-musl"),
            "berth-x86_64-unknown-linux-musl.tar.gz"
        );
    }
}
