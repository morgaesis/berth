//! Single-round-trip remote probe.
//!
//! The probe command is deliberately conservative POSIX-sh: only `uname`,
//! `printf`, `command -v`, and `$HOME`/`$PATH` expansions, all available
//! on busybox, dash, ash, bash, and zsh. The output is a simple KEY=VALUE
//! list which we parse strictly client-side.

use anyhow::{Context, Result};

/// Snapshot of a remote host's identity and any existing berth install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEnv {
    pub os: String,
    pub arch: String,
    pub berth_path: Option<String>,
    pub berth_version: Option<String>,
    pub berth_build: Option<String>,
    pub home: String,
    pub path_env: String,
}

// Prefer the path Berth deploys to. A PATH-provided binary may exist, but
// it is not the binary this client controls and may be stale.
pub(crate) const PROBE_SCRIPT: &str = "\
printf 'OS=%s\\n' \"$(uname -s 2>/dev/null || echo unknown)\"; \
printf 'ARCH=%s\\n' \"$(uname -m 2>/dev/null || echo unknown)\"; \
printf 'HOME=%s\\n' \"$HOME\"; \
printf 'PATH=%s\\n' \"$PATH\"; \
berth_bin=; \
if [ -x \"$HOME/.local/bin/berth\" ]; then \
  berth_bin=\"$HOME/.local/bin/berth\"; \
elif command -v berth >/dev/null 2>&1; then \
  berth_bin=$(command -v berth); \
fi; \
if [ -n \"$berth_bin\" ]; then \
  printf 'BERTH_PATH=%s\\n' \"$berth_bin\"; \
  v=$({ \"$berth_bin\" --version 2>/dev/null || true; } | awk 'NR==1{for(i=1;i<=NF;i++) if($i ~ /^[0-9]+\\.[0-9]+\\.[0-9]+/) {print $i; exit}}'); \
  [ -n \"$v\" ] && printf 'BERTH_VERSION=%s\\n' \"$v\"; \
  b=$({ \"$berth_bin\" version-info 2>/dev/null || true; } | awk -F= '$1==\"BUILD\" {print $2; exit}'); \
  [ -n \"$b\" ] && printf 'BERTH_BUILD=%s\\n' \"$b\"; \
fi; \
exit 0";

/// Run the probe over SSH and parse the result.
#[tracing::instrument(level = "debug", skip(host), fields(host = %host))]
pub async fn probe(host: &str) -> Result<RemoteEnv> {
    tracing::debug!(script_len = PROBE_SCRIPT.len(), "running probe over ssh");
    let raw = crate::ssh::run_remote_command(host, PROBE_SCRIPT)
        .await
        .with_context(|| format!("probing {host} over SSH"))?;
    tracing::debug!(raw_lines = raw.lines().count(), "probe ssh returned");
    let env = parse(&raw)?;
    tracing::info!(
        os = %env.os, arch = %env.arch,
        existing_berth = ?env.berth_version,
        existing_build = ?env.berth_build,
        "probe complete"
    );
    Ok(env)
}

fn parse(raw: &str) -> Result<RemoteEnv> {
    let mut os = None;
    let mut arch = None;
    let mut home = None;
    let mut path_env = None;
    let mut berth_path = None;
    let mut berth_version = None;
    let mut berth_build = None;
    for line in raw.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "OS" => os = Some(value.to_string()),
            "ARCH" => arch = Some(value.to_string()),
            "HOME" => home = Some(value.to_string()),
            "PATH" => path_env = Some(value.to_string()),
            "BERTH_PATH" => berth_path = Some(value.to_string()),
            "BERTH_VERSION" => berth_version = Some(value.to_string()),
            "BERTH_BUILD" => berth_build = sanitize_build_id(value),
            _ => {}
        }
    }
    Ok(RemoteEnv {
        os: os.context("probe output missing OS=")?,
        arch: arch.context("probe output missing ARCH=")?,
        berth_path,
        berth_version,
        berth_build,
        home: home.unwrap_or_default(),
        path_env: path_env.unwrap_or_default(),
    })
}

fn sanitize_build_id(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 80
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
    .then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::process::Command;

    #[test]
    fn parse_minimal() {
        let raw = "OS=Linux\nARCH=x86_64\nHOME=/home/me\nPATH=/usr/bin\n";
        let env = parse(raw).unwrap();
        assert_eq!(env.os, "Linux");
        assert_eq!(env.arch, "x86_64");
        assert_eq!(env.home, "/home/me");
        assert!(env.berth_path.is_none());
        assert!(env.berth_version.is_none());
        assert!(env.berth_build.is_none());
    }

    #[test]
    fn parse_with_existing_berth() {
        let raw = "OS=Linux\nARCH=aarch64\nHOME=/home/me\nPATH=/usr/bin:/home/me/.local/bin\nBERTH_PATH=/home/me/.local/bin/berth\nBERTH_VERSION=0.1.0\nBERTH_BUILD=abc123\n";
        let env = parse(raw).unwrap();
        assert_eq!(env.berth_version.as_deref(), Some("0.1.0"));
        assert_eq!(env.berth_build.as_deref(), Some("abc123"));
        assert_eq!(env.berth_path.as_deref(), Some("/home/me/.local/bin/berth"));
    }

    #[test]
    fn parse_ignores_unknown_keys_and_blank_lines() {
        let raw = "FOO=bar\n\nOS=Linux\nARCH=x86_64\nHOME=/x\nPATH=/x\nGARBAGE\n";
        let env = parse(raw).unwrap();
        assert_eq!(env.os, "Linux");
    }

    #[test]
    fn parse_fails_without_os() {
        let raw = "ARCH=x86_64\nHOME=/x\nPATH=/x\n";
        assert!(parse(raw).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn probe_script_tolerates_stale_berth_without_version_info() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bin_dir = temp.path().join(".local").join("bin");
        fs::create_dir_all(&bin_dir).expect("create fake bin dir");
        let berth = bin_dir.join("berth");
        fs::write(
            &berth,
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'berth 0.1.13'; exit 0; fi\nexit 1\n",
        )
        .expect("write fake berth");
        let mut perms = fs::metadata(&berth)
            .expect("fake berth metadata")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&berth, perms).expect("chmod fake berth");

        let output = Command::new("sh")
            .arg("-c")
            .arg(PROBE_SCRIPT)
            .env("HOME", temp.path())
            .env("PATH", "/usr/bin:/bin")
            .output()
            .expect("run probe script");

        assert!(
            output.status.success(),
            "probe script failed: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("BERTH_VERSION=0.1.13"), "{stdout}");
        assert!(!stdout.contains("BERTH_BUILD="), "{stdout}");
        let parsed = parse(&stdout).expect("parse probe output");
        assert_eq!(parsed.berth_version.as_deref(), Some("0.1.13"));
        assert!(parsed.berth_build.is_none());
    }
}
