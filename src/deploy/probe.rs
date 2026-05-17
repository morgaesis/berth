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
    pub home: String,
    pub path_env: String,
}

// `command -v berth` alone is insufficient: `~/.local/bin` is on most
// users' interactive-shell PATH (Ubuntu's `.profile` adds it) but NOT on
// the PATH a non-interactive `ssh host cmd` session sees. We deployed to
// `~/.local/bin/berth`, so we explicitly probe that path as a fallback.
const PROBE_SCRIPT: &str = "\
printf 'OS=%s\\n' \"$(uname -s 2>/dev/null || echo unknown)\"; \
printf 'ARCH=%s\\n' \"$(uname -m 2>/dev/null || echo unknown)\"; \
printf 'HOME=%s\\n' \"$HOME\"; \
printf 'PATH=%s\\n' \"$PATH\"; \
berth_bin=; \
if command -v berth >/dev/null 2>&1; then \
  berth_bin=$(command -v berth); \
elif [ -x \"$HOME/.local/bin/berth\" ]; then \
  berth_bin=\"$HOME/.local/bin/berth\"; \
fi; \
if [ -n \"$berth_bin\" ]; then \
  printf 'BERTH_PATH=%s\\n' \"$berth_bin\"; \
  v=$(\"$berth_bin\" --version 2>/dev/null | awk 'NR==1{for(i=1;i<=NF;i++) if($i ~ /^[0-9]+\\.[0-9]+\\.[0-9]+/) {print $i; exit}}'); \
  [ -n \"$v\" ] && printf 'BERTH_VERSION=%s\\n' \"$v\"; \
fi";

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
            _ => {}
        }
    }
    Ok(RemoteEnv {
        os: os.context("probe output missing OS=")?,
        arch: arch.context("probe output missing ARCH=")?,
        berth_path,
        berth_version,
        home: home.unwrap_or_default(),
        path_env: path_env.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal() {
        let raw = "OS=Linux\nARCH=x86_64\nHOME=/home/me\nPATH=/usr/bin\n";
        let env = parse(raw).unwrap();
        assert_eq!(env.os, "Linux");
        assert_eq!(env.arch, "x86_64");
        assert_eq!(env.home, "/home/me");
        assert!(env.berth_path.is_none());
        assert!(env.berth_version.is_none());
    }

    #[test]
    fn parse_with_existing_berth() {
        let raw = "OS=Linux\nARCH=aarch64\nHOME=/home/me\nPATH=/usr/bin:/home/me/.local/bin\nBERTH_PATH=/home/me/.local/bin/berth\nBERTH_VERSION=0.1.0\n";
        let env = parse(raw).unwrap();
        assert_eq!(env.berth_version.as_deref(), Some("0.1.0"));
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
}
