//! Deploy by having the *remote* download the release asset directly from
//! GitHub, instead of pulling it locally and pushing it over scp.
//!
//! This is the preferred release-deploy path: the remote usually has far
//! better bandwidth to GitHub than the local↔remote SSH link (especially
//! from a laptop on VPN), and it works even when the local platform has no
//! release artifact of its own (e.g. a Windows client deploying to Linux).
//! Verification mirrors `fetch.rs`: the sha256 sidecar is downloaded next
//! to the archive and checked on the remote before the binary is moved
//! into place. Any failure — no curl/wget, no sha tool, 404, checksum
//! mismatch — surfaces as an error so the caller can fall back to the
//! local download + scp proxy path.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::time::Duration;

use super::fetch::{asset_name, base_url};

/// Set to opt out of the remote-direct download and always proxy the
/// artifact through the local machine (useful behind egress-restricted
/// remotes or in tests).
const NO_REMOTE_FETCH_ENV: &str = "BERTH_DEPLOY_NO_REMOTE_FETCH";

/// Generous ceiling for download + checksum + extract on the remote; the
/// musl artifact is ~10 MiB so this only matters on truly slow egress.
const REMOTE_FETCH_TIMEOUT: Duration = Duration::from_secs(180);

pub fn remote_fetch_enabled() -> bool {
    // The BERTH_SKIP_SSH harness stubs run_remote_command with a canned
    // success string; treating that as a completed download would skip the
    // legacy path the tests actually exercise.
    std::env::var_os(NO_REMOTE_FETCH_ENV).is_none()
        && std::env::var_os("BERTH_SKIP_SSH").is_none()
}

/// Download + verify + install the release asset for `tag`/`target` *on*
/// `host`, returning the expanded remote install path. The script is
/// self-contained POSIX sh; every interpolated value is validated or from
/// a fixed internal set, so no quoting gymnastics are needed.
pub async fn deploy_via_remote_download(
    host: &str,
    tag: &str,
    target: &str,
) -> Result<PathBuf> {
    let tag_path = tag.trim_start_matches('v');
    crate::validate_release_tag(tag_path)?;
    let asset = asset_name(target);
    let base = base_url()?;
    let bin_url = format!("{base}/v{tag_path}/{asset}");
    let sha_url = format!("{bin_url}.sha256");

    let script = remote_fetch_script(&asset, &bin_url, &sha_url);
    tracing::info!(host = %host, bin_url = %bin_url, "remote-direct release download");
    let out = crate::ssh::run_remote_command_with_timeout(host, &script, REMOTE_FETCH_TIMEOUT)
        .await
        .with_context(|| format!("remote-direct download of {asset} on {host}"))?;
    let path = out.trim();
    if path.is_empty() {
        bail!("remote-direct download reported no install path");
    }
    Ok(PathBuf::from(path))
}

/// The sidecar is saved next to the archive under the asset's real
/// filename so a plain `sha256sum -c` (or `shasum -a 256 -c` on macOS)
/// validates it without any awk/cut parsing.
fn remote_fetch_script(asset: &str, bin_url: &str, sha_url: &str) -> String {
    format!(
        r#"set -eu
asset="{asset}"
tmp="${{TMPDIR:-/tmp}}/berth-deploy-$$"
mkdir -p "$tmp"
cd "$tmp"
if command -v curl >/dev/null 2>&1; then
  curl -fsSL -o "$asset" {bin_url}
  curl -fsSL -o "$asset.sha256" {sha_url}
elif command -v wget >/dev/null 2>&1; then
  wget -q -O "$asset" {bin_url}
  wget -q -O "$asset.sha256" {sha_url}
else
  echo "berth: no curl or wget on remote" >&2
  exit 90
fi
if command -v sha256sum >/dev/null 2>&1; then
  sha256sum -c "$asset.sha256" >/dev/null
elif command -v shasum >/dev/null 2>&1; then
  shasum -a 256 -c "$asset.sha256" >/dev/null
else
  echo "berth: no sha256sum or shasum on remote" >&2
  exit 91
fi
mkdir -p "$HOME/.local/bin"
tar -xzOf "$asset" berth > "$HOME/.local/bin/berth.new"
chmod +x "$HOME/.local/bin/berth.new"
mv "$HOME/.local/bin/berth.new" "$HOME/.local/bin/berth"
cd /
rm -rf "$tmp"
printf %s "$HOME/.local/bin/berth"
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_embeds_urls_and_moves_atomically() {
        let script = remote_fetch_script(
            "berth-x86_64-unknown-linux-musl.tar.gz",
            "https://example.com/v0.1.14/berth-x86_64-unknown-linux-musl.tar.gz",
            "https://example.com/v0.1.14/berth-x86_64-unknown-linux-musl.tar.gz.sha256",
        );
        assert!(script.contains("curl -fsSL"));
        assert!(script.contains("wget -q"));
        assert!(script.contains("sha256sum -c"));
        // Write-then-mv so a running berth never sees a half-written ELF.
        assert!(script.contains("berth.new"));
        assert!(script.contains(r#"mv "$HOME/.local/bin/berth.new" "$HOME/.local/bin/berth""#));
        assert!(script.contains(".tar.gz.sha256"));
    }

    #[test]
    fn rejects_invalid_tag_before_touching_the_network() {
        assert!(crate::validate_release_tag("0.1.14; rm -rf /").is_err());
    }
}
