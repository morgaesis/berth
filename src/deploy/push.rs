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
#[tracing::instrument(level = "debug", skip(host, local_bin), fields(host = %host, local_bin = %local_bin.display()))]
pub async fn push_binary(host: &str, local_bin: &Path) -> Result<PathBuf> {
    // Step 1: ensure the destination directory exists on the remote, and
    //         pre-emptively remove the target file. Some sshfs/sftp
    //         implementations refuse to overwrite a currently-running ELF
    //         executable (ETXTBSY-style failure), so we `rm -f` first;
    //         Linux unlink on a busy file just detaches the inode while
    //         the running process keeps its mapping. The next scp lays
    //         down a fresh file at the same path.
    tracing::debug!("ensuring remote ~/.local/bin exists and target is removable");
    let prep_status = Command::new("ssh")
        .arg(host)
        .arg("mkdir -p ~/.local/bin && rm -f ~/.local/bin/berth")
        .status()
        .await
        .context("invoking ssh to prepare remote ~/.local/bin/berth")?;
    if !prep_status.success() {
        bail!("ssh {host} prep step exited {prep_status}");
    }

    // Step 2: scp the binary across. scp accepts `~/...` on the remote
    //         side and resolves it via the user's login shell.
    let dest = format!("{host}:{}", remote_install_path());
    tracing::debug!(dest = %dest, "running scp");
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
    tracing::debug!("scp completed");

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
    tracing::debug!("chmod +x done");

    // Step 4: resolve `~/.local/bin/berth` to an absolute path so the
    //         caller can use it as a stable reference in subsequent
    //         smoke tests and config records.
    let resolved = crate::ssh::run_remote_command(host, "printf %s \"$HOME/.local/bin/berth\"")
        .await
        .context("resolving remote ~/.local/bin/berth")?;
    let path = PathBuf::from(resolved.trim());
    tracing::info!(remote_path = %path.display(), "binary in place on remote");
    Ok(path)
}
