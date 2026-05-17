//! Push a local berth binary to a remote host via system `scp`.
//!
//! We deliberately shell out to `scp` (and `ssh` for the post-mkdir +
//! chmod) rather than depend on a Rust SSH crate. `scp` is universally
//! available wherever interactive `ssh` is set up; modern OpenSSH
//! routes it through SFTP transparently. This keeps the dependency
//! footprint tiny and reuses the user's existing ssh config, agent,
//! and known_hosts.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Standard install location on the remote: per-user, on PATH for most
/// modern Linux setups out of the box. Sudo is intentionally out of scope.
pub fn remote_install_path() -> &'static str {
    "~/.local/bin/berth"
}

/// Copy `local_bin` to the remote, chmod +x it, and return the *expanded*
/// remote path (i.e. with `~` resolved to the remote `$HOME` on disk).
pub async fn push_binary(host: &str, local_bin: &Path) -> Result<PathBuf> {
    // Step 1: ensure the destination directory exists on the remote.
    //         Done via ssh (mkdir -p ~/.local/bin) so we don't rely on
    //         scp's mkdir-on-write extension.
    let mkdir_status = Command::new("ssh")
        .arg(host)
        .arg("mkdir -p ~/.local/bin")
        .status()
        .await
        .context("invoking ssh to create remote ~/.local/bin")?;
    if !mkdir_status.success() {
        bail!("ssh {host} mkdir -p ~/.local/bin exited {mkdir_status}");
    }

    // Step 2: scp the binary across. scp accepts `~/...` on the remote
    //         side and resolves it via the user's login shell.
    let dest = format!("{host}:{}", remote_install_path());
    let scp_status = Command::new("scp")
        .arg("-q")
        .arg(local_bin)
        .arg(&dest)
        .status()
        .await
        .with_context(|| format!("invoking scp {} {dest}", local_bin.display()))?;
    if !scp_status.success() {
        bail!("scp to {dest} exited {scp_status}");
    }

    // Step 3: chmod +x — scp preserves source mode but be defensive in
    //         case a future caller passes a non-executable cache file.
    let chmod_status = Command::new("ssh")
        .arg(host)
        .arg("chmod +x ~/.local/bin/berth")
        .status()
        .await
        .context("invoking ssh to chmod the deployed binary")?;
    if !chmod_status.success() {
        bail!("ssh {host} chmod +x exited {chmod_status}");
    }

    // Step 4: resolve `~/.local/bin/berth` to an absolute path so the
    //         caller can use it as a stable reference in subsequent
    //         smoke tests and config records.
    let resolved = crate::ssh::run_remote_command(host, "printf %s \"$HOME/.local/bin/berth\"")
        .await
        .context("resolving remote ~/.local/bin/berth")?;
    Ok(PathBuf::from(resolved.trim()))
}
