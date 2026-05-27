//! Remote-host probing and binary deployment.
//!
//! `probe` runs a single, idempotent SSH command returning KEY=VALUE pairs
//! identifying the remote OS/architecture and any existing berth install.
//! `ensure_deployed` orchestrates probe → fetch matching musl-static
//! binary from this project's GitHub releases → `scp` to `~/.local/bin/berth`
//! on the remote → smoke-test → record consent in config.
//!
//! Why a separate module:
//! - Pure remote-IO contract; isolated from the config / cli / runtime
//!   layers so it can be exercised with a `BERTH_SKIP_SSH` harness.
//! - Future "embed local musl binaries via include_bytes!" can drop in
//!   behind the same public API (`fetch_binary`).

pub mod fetch;
pub mod probe;
pub mod push;

use anyhow::{bail, Context, Result};
use std::path::PathBuf;

pub use fetch::fetch_binary;
pub use probe::{probe, RemoteEnv};
pub use push::push_binary;

pub mod freshness;
pub mod local;

/// Options that control whether `ensure_deployed` is allowed to actually
/// write to the remote. Maps to the user-facing `--auto-deploy` /
/// `--no-deploy` flags plus the per-host trust persisted in config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentMode {
    /// User explicitly authorized this run (`--auto-deploy` or
    /// `berth deploy <host>` invoked directly).
    AutoApproved,
    /// User explicitly forbade deploying (`--no-deploy`).
    Forbidden,
    /// Default: ask once via prompt; persist on accept.
    Ask,
}

/// Map `uname -s` + `uname -m` to a Rust target triple matching a release
/// asset we actually publish. Keep this list in sync with the matrix in
/// `.github/workflows/release.yml` — an arch listed here that isn't built
/// would lead the deploy path to 404 confidently.
pub fn target_triple(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("Linux", "x86_64") => Some("x86_64-unknown-linux-musl"),
        ("Linux", "aarch64") | ("Linux", "arm64") => Some("aarch64-unknown-linux-musl"),
        ("Linux", "armv7l") | ("Linux", "armv7") | ("Linux", "armhf") => {
            Some("armv7-unknown-linux-musleabihf")
        }
        ("Darwin", "arm64") | ("Darwin", "aarch64") => Some("aarch64-apple-darwin"),
        // No Intel-Mac binary in the release matrix; users on
        // `x86_64-apple-darwin` get a clear "unsupported arch" error
        // instead of a 404 fetch.
        _ => None,
    }
}

/// Decide what action ensure_deployed should take given a probe result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployDecision {
    /// Remote already has a compatible berth at the expected version; nothing to do.
    UpToDate,
    /// No berth on the remote, or version mismatch.
    Deploy {
        target: &'static str,
        reason: String,
        source: DeploySource,
    },
    /// Local build differs but cannot be pushed safely to this remote target.
    LocalBuildUnsupported {
        target: &'static str,
        local_target: Option<&'static str>,
        reason: String,
    },
    /// Remote architecture is not in our build matrix.
    UnsupportedArch { os: String, arch: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeploySource {
    Release,
    LocalBinary,
}

/// Compare remote vs local versions with semver semantics: deploy only when
/// the local berth is *strictly newer* than the remote, so a host running
/// a future version doesn't get silently downgraded. If either side fails
/// to parse as semver, fall back to string equality.
pub fn decide(env: &RemoteEnv, local_version: &str, local_build: &str) -> DeployDecision {
    let Some(target) = target_triple(&env.os, &env.arch) else {
        return DeployDecision::UnsupportedArch {
            os: env.os.clone(),
            arch: env.arch.clone(),
        };
    };
    let Some(remote_ver) = env.berth_version.as_deref() else {
        let source = if local_binary_compatible_with(target) {
            DeploySource::LocalBinary
        } else {
            DeploySource::Release
        };
        return DeployDecision::Deploy {
            target,
            reason: "remote has no berth binary".to_string(),
            source,
        };
    };
    let (local_parsed, remote_parsed) = (
        semver::Version::parse(local_version),
        semver::Version::parse(remote_ver),
    );
    match (local_parsed, remote_parsed) {
        (Ok(local), Ok(remote)) if local < remote => DeployDecision::UpToDate,
        (Ok(local), Ok(remote)) if local == remote => {
            decide_same_version_build(env, target, local_build, &remote.to_string())
        }
        (Ok(local), Ok(remote)) => DeployDecision::Deploy {
            target,
            reason: format!("remote has berth {remote}, local is {local} (newer)"),
            source: DeploySource::Release,
        },
        _ if remote_ver == local_version => {
            decide_same_version_build(env, target, local_build, remote_ver)
        }
        _ => DeployDecision::Deploy {
            target,
            reason: format!(
                "remote berth {remote_ver} could not be compared to local {local_version}, or build ids differ"
            ),
            source: DeploySource::Release,
        },
    }
}

fn decide_same_version_build(
    env: &RemoteEnv,
    target: &'static str,
    local_build: &str,
    version: &str,
) -> DeployDecision {
    if env.berth_build.as_deref() == Some(local_build) {
        return DeployDecision::UpToDate;
    }

    let reason = match env.berth_build.as_deref() {
        Some(remote_build) => {
            format!("remote has berth {version} build {remote_build}, local build is {local_build}")
        }
        None => format!("remote has berth {version} but no build id; local build is {local_build}"),
    };
    let local_target = Some(crate::build_info::build_target());
    if local_binary_compatible_with(target) {
        DeployDecision::Deploy {
            target,
            reason,
            source: DeploySource::LocalBinary,
        }
    } else {
        DeployDecision::LocalBuildUnsupported {
            target,
            local_target,
            reason,
        }
    }
}

pub fn local_binary_compatible_with(remote_target: &str) -> bool {
    let local = crate::build_info::build_target();
    if local == remote_target {
        return true;
    }
    let Some((local_arch, local_os, _)) = split_target(local) else {
        return false;
    };
    let Some((remote_arch, remote_os, _)) = split_target(remote_target) else {
        return false;
    };
    local_arch == remote_arch && local_os == remote_os && matches!(local_os, "linux" | "darwin")
}

fn split_target(target: &str) -> Option<(&str, &str, &str)> {
    let parts: Vec<&str> = target.split('-').collect();
    Some((
        *parts.first()?,
        *parts.get(2)?,
        *parts.get(3).unwrap_or(&""),
    ))
}

/// Run the full deploy: fetch + push + smoke-test. Caller is responsible
/// for consent gating before invoking this. Status output goes to stderr
/// via indicatif so a quiet network doesn't look like a hung process.
#[tracing::instrument(level = "info", skip(host), fields(host = %host, tag = %tag, target = %target))]
pub async fn ensure_deployed(host: &str, tag: &str, target: &'static str) -> Result<DeployedInfo> {
    tracing::info!("starting deploy");
    // fetch_binary already renders its own bytes/Content-Length bar.
    let local = fetch_binary(tag, target).await?;
    tracing::info!(local_path = %local.display(), "fetched binary");

    let scp_bar = phase_spinner(&format!("scp to {host}"));
    let remote_path = push_binary(host, &local).await?;
    scp_bar.finish_and_clear();
    tracing::info!(remote_path = %remote_path.display(), "pushed binary");

    let smoke_bar = phase_spinner("smoke-test");
    smoke_test(host, &remote_path, tag.trim_start_matches('v'), None).await?;
    smoke_bar.finish_and_clear();
    tracing::info!("smoke-test ok");

    Ok(DeployedInfo {
        remote_path,
        target: target.to_string(),
        version: tag.trim_start_matches('v').to_string(),
    })
}

pub async fn ensure_deployed_local(host: &str, target: &'static str) -> Result<DeployedInfo> {
    tracing::info!("starting local binary deploy");
    let local = std::env::current_exe().context("resolving current berth executable")?;
    let scp_bar = phase_spinner(&format!("scp local build to {host}"));
    let remote_path = push_binary(host, &local).await?;
    scp_bar.finish_and_clear();

    let smoke_bar = phase_spinner("smoke-test");
    smoke_test(
        host,
        &remote_path,
        crate::build_info::version(),
        Some(crate::build_info::build_id()),
    )
    .await?;
    smoke_bar.finish_and_clear();
    tracing::info!("local binary smoke-test ok");

    Ok(DeployedInfo {
        remote_path,
        target: target.to_string(),
        version: crate::build_info::version().to_string(),
    })
}

fn phase_spinner(message: &str) -> indicatif::ProgressBar {
    use indicatif::{ProgressBar, ProgressStyle};
    use std::time::Duration;
    let pb = ProgressBar::new_spinner();
    pb.enable_steady_tick(Duration::from_millis(80));
    pb.set_style(
        ProgressStyle::with_template("  {msg:<28.cyan} {spinner}")
            .unwrap()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "),
    );
    pb.set_message(message.to_string());
    pb
}

/// Result of a successful deploy. Returned by `ensure_deployed` and
/// consumed by callers that want to record a `TrustedHost` entry in the
/// user's config.
#[derive(Debug, Clone)]
pub struct DeployedInfo {
    /// Absolute path on the remote (`~`-expanded) where the binary lives.
    pub remote_path: PathBuf,
    /// The Rust target triple that was deployed.
    pub target: String,
    /// The version that's now on the remote (no leading `v`).
    pub version: String,
}

/// Insert/refresh a `TrustedHost` entry for `host` based on a successful
/// deploy. Persists the config to disk.
pub fn record_trust(
    config: &mut crate::config::Config,
    host: &str,
    info: &DeployedInfo,
) -> Result<()> {
    config.trusted_hosts.insert(
        host.to_string(),
        crate::config::TrustedHost {
            target: info.target.clone(),
            version: info.version.clone(),
            remote_path: info.remote_path.to_string_lossy().to_string(),
        },
    );
    config.save()?;
    Ok(())
}

#[tracing::instrument(level = "debug", skip(host, remote_path), fields(host = %host, remote_path = %remote_path.display()))]
async fn smoke_test(
    host: &str,
    remote_path: &std::path::Path,
    expected_version: &str,
    expected_build: Option<&str>,
) -> Result<()> {
    let path_str = remote_path.to_string_lossy().to_string();
    let quoted = path_str.replace('\'', "'\"'\"'");
    let cmd = if expected_build.is_some() {
        format!("'{}' --version && '{}' version-info", quoted, quoted)
    } else {
        format!("'{}' --version", quoted)
    };
    tracing::debug!(cmd = %cmd, "running --version over ssh");
    let out = crate::ssh::run_remote_command(host, &cmd)
        .await
        .with_context(|| format!("running `{path_str} --version` on {host}"))?;
    tracing::debug!(output = %out.trim(), "smoke-test output");
    if !out.contains("berth") || !out.contains(expected_version) {
        bail!(
            "deployed binary at {} did not report expected version {}; output: {}",
            path_str,
            expected_version,
            out.trim()
        );
    }
    if let Some(expected_build) = expected_build {
        let expected_line = format!("BUILD={expected_build}");
        if !out.lines().any(|line| line.trim() == expected_line) {
            bail!(
                "deployed binary at {} did not report expected build {}; output: {}",
                path_str,
                expected_build,
                out.trim()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_triple_maps_common_archs() {
        assert_eq!(
            target_triple("Linux", "x86_64"),
            Some("x86_64-unknown-linux-musl")
        );
        assert_eq!(
            target_triple("Linux", "aarch64"),
            Some("aarch64-unknown-linux-musl")
        );
        assert_eq!(
            target_triple("Linux", "armv7l"),
            Some("armv7-unknown-linux-musleabihf")
        );
        assert_eq!(
            target_triple("Darwin", "arm64"),
            Some("aarch64-apple-darwin")
        );
        // Intel Mac is intentionally absent from the release matrix; the
        // probe path must surface UnsupportedArch rather than a 404.
        assert_eq!(target_triple("Darwin", "x86_64"), None);
        assert_eq!(target_triple("Plan9", "amd64"), None);
    }

    #[test]
    fn decide_up_to_date() {
        let env = RemoteEnv {
            os: "Linux".into(),
            arch: "x86_64".into(),
            berth_path: Some("/home/me/.local/bin/berth".into()),
            berth_version: Some("0.1.0".into()),
            berth_build: Some("same".into()),
            home: "/home/me".into(),
            path_env: "/usr/bin:/home/me/.local/bin".into(),
        };
        assert_eq!(decide(&env, "0.1.0", "same"), DeployDecision::UpToDate);
    }

    #[test]
    fn decide_unsupported_arch_short_circuits() {
        let env = RemoteEnv {
            os: "Plan9".into(),
            arch: "amd64".into(),
            berth_path: None,
            berth_version: None,
            berth_build: None,
            home: "/usr/me".into(),
            path_env: "/bin".into(),
        };
        match decide(&env, "0.1.0", "same") {
            DeployDecision::UnsupportedArch { os, arch } => {
                assert_eq!(os, "Plan9");
                assert_eq!(arch, "amd64");
            }
            other => panic!("expected UnsupportedArch, got {other:?}"),
        }
    }

    #[test]
    fn decide_deploys_when_remote_missing() {
        let local_target = crate::build_info::build_target();
        let (os, arch) = if local_target.contains("linux") && local_target.starts_with("x86_64-") {
            ("Linux", "x86_64")
        } else if local_target.contains("linux") && local_target.starts_with("aarch64-") {
            ("Linux", "aarch64")
        } else if local_target.contains("darwin") && local_target.starts_with("aarch64-") {
            ("Darwin", "arm64")
        } else {
            ("Linux", "aarch64")
        };
        let env = RemoteEnv {
            os: os.into(),
            arch: arch.into(),
            berth_path: None,
            berth_version: None,
            berth_build: None,
            home: "/home/me".into(),
            path_env: "/usr/bin".into(),
        };
        match decide(&env, "0.1.0", "same") {
            DeployDecision::Deploy { source, .. }
                if local_binary_compatible_with(target_triple(os, arch).unwrap()) =>
            {
                assert_eq!(source, DeploySource::LocalBinary);
            }
            DeployDecision::Deploy { source, .. } => {
                assert_eq!(source, DeploySource::Release);
            }
            other => panic!("expected Deploy, got {other:?}"),
        }
    }

    #[test]
    fn decide_does_not_downgrade_a_newer_remote() {
        // If the remote happens to be ahead of us, leave it alone.
        let env = RemoteEnv {
            os: "Linux".into(),
            arch: "x86_64".into(),
            berth_path: Some("/r/.local/bin/berth".into()),
            berth_version: Some("0.2.0".into()),
            berth_build: Some("remote".into()),
            home: "/r".into(),
            path_env: "/bin".into(),
        };
        assert_eq!(decide(&env, "0.1.0", "local"), DeployDecision::UpToDate);
    }

    #[test]
    fn decide_deploys_when_local_is_strictly_newer() {
        let env = RemoteEnv {
            os: "Linux".into(),
            arch: "x86_64".into(),
            berth_path: Some("/r/.local/bin/berth".into()),
            berth_version: Some("0.1.0".into()),
            berth_build: Some("old".into()),
            home: "/r".into(),
            path_env: "/bin".into(),
        };
        match decide(&env, "0.2.0", "new") {
            DeployDecision::Deploy { target, reason, .. } => {
                assert_eq!(target, "x86_64-unknown-linux-musl");
                assert!(
                    reason.contains("newer"),
                    "reason should mention upgrade: {reason}"
                );
            }
            other => panic!("expected Deploy, got {other:?}"),
        }
    }

    #[test]
    fn decide_deploys_when_same_version_build_differs() {
        let env = RemoteEnv {
            os: "Linux".into(),
            arch: "x86_64".into(),
            berth_path: Some("/r/.local/bin/berth".into()),
            berth_version: Some("0.1.0".into()),
            berth_build: Some("old".into()),
            home: "/r".into(),
            path_env: "/bin".into(),
        };
        match decide(&env, "0.1.0", "new") {
            DeployDecision::Deploy { reason, source, .. } => {
                assert!(
                    reason.contains("build"),
                    "reason should mention build: {reason}"
                );
                assert_eq!(source, DeploySource::LocalBinary);
            }
            DeployDecision::LocalBuildUnsupported { reason, .. } => {
                assert!(
                    reason.contains("build"),
                    "reason should mention build: {reason}"
                );
            }
            other => panic!("expected Deploy, got {other:?}"),
        }
    }
}
