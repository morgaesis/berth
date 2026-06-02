//! `berth logs` — dump recent berth activity so the user can share state
//! when something goes wrong. Two sources:
//!
//!   1. The global berth log at `$XDG_STATE_HOME/berth/log/berth.log`
//!      (everything tracing-info+ from this binary across all invocations).
//!   2. Per-session supervisor logs at
//!      `$XDG_RUNTIME_DIR/berth/sessions/<workspace>/<id>.log` (captures
//!      the PTY child's stdout+stderr; this is where a failed `bash -lc`
//!      command's error text ends up).

use anyhow::{Context, Result};
use berth::config::Config;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const DEFAULT_LINES: usize = 200;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "trace" | "TRACE" => Some(Self::Trace),
            "debug" | "DEBUG" => Some(Self::Debug),
            "info" | "INFO" => Some(Self::Info),
            "warn" | "WARN" => Some(Self::Warn),
            "error" | "ERROR" => Some(Self::Error),
            _ => None,
        }
    }
}

fn global_log_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("BERTH_LOG_FILE") {
        return Some(PathBuf::from(p));
    }
    let base = std::env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .ok()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("state")))?;
    Some(base.join("berth").join("log").join("berth.log"))
}

pub async fn run(
    lines: Option<usize>,
    follow: bool,
    sessions: bool,
    level: Option<&str>,
    remote: Option<&str>,
) -> Result<()> {
    let n = lines.unwrap_or(DEFAULT_LINES);
    let min_level = level.and_then(LogLevel::parse);
    let mut follow_paths = Vec::new();

    if let Some(path) = global_log_path() {
        if path.exists() {
            println!("=== {} (last {n} lines) ===", path.display());
            print_tail(&path, n, min_level)?;
        } else {
            println!("(no global log at {})", path.display());
        }
        follow_paths.push(path);
    }

    if sessions || lines.is_none() {
        let runtime = berth::session::runtime_dir()?;
        let sessions_root = runtime.join("sessions");
        if sessions_root.exists() {
            for entry in fs::read_dir(&sessions_root)
                .with_context(|| format!("reading {}", sessions_root.display()))?
            {
                let ws_dir = entry?.path();
                if !ws_dir.is_dir() {
                    continue;
                }
                for log in fs::read_dir(&ws_dir)? {
                    let log = log?.path();
                    if log.extension().and_then(|s| s.to_str()) != Some("log") {
                        continue;
                    }
                    println!("\n=== {} (last {n} lines) ===", log.display());
                    print_tail(&log, n, min_level)?;
                    follow_paths.push(log);
                }
            }
        }
    }

    if let Some(target) = remote {
        if follow {
            eprintln!("berth logs: --remote is fetched once; --follow only follows local logs");
        }
        let host = resolve_remote_target(target)?;
        println!("\n=== remote {host} ===");
        print!(
            "{}",
            berth::ssh::run_remote_command(&host, &remote_logs_command(n, level, sessions)).await?
        );
    }
    if follow {
        follow_logs(&follow_paths, min_level).await?;
    }
    Ok(())
}

fn resolve_remote_target(target: &str) -> Result<String> {
    let config = Config::load()?;
    if let Some(ws) = config.workspaces.get(target) {
        return config
            .resolved_remote(target, ws)
            .with_context(|| format!("workspace '{target}' has no remote host"));
    }
    berth::validate_ssh_host(target)?;
    Ok(target.to_string())
}

fn remote_logs_command(lines: usize, level: Option<&str>, sessions: bool) -> String {
    let mut remote = String::from(
        "berth_bin=; if [ -x \"$HOME/.local/bin/berth\" ]; then berth_bin=\"$HOME/.local/bin/berth\"; elif command -v berth >/dev/null 2>&1; then berth_bin=$(command -v berth); else printf 'berth not found on remote\\n' >&2; exit 127; fi; BERTH_ATTACH_LOCAL=1 exec \"$berth_bin\" logs",
    );
    remote.push_str(" -n ");
    remote.push_str(&lines.to_string());
    if sessions {
        remote.push_str(" --sessions");
    }
    if let Some(level) = level {
        remote.push_str(" --level ");
        remote.push_str(&berth::ssh::shell_escape_arg(level));
    }
    remote
}

async fn follow_logs(paths: &[PathBuf], min_level: Option<LogLevel>) -> Result<()> {
    let mut offsets = Vec::with_capacity(paths.len());
    for path in paths {
        offsets.push(current_len(path));
    }

    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        for (path, offset) in paths.iter().zip(offsets.iter_mut()) {
            let len = current_len(path);
            if len < *offset {
                *offset = 0;
            }
            if len <= *offset {
                continue;
            }

            let mut f =
                fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
            f.seek(SeekFrom::Start(*offset))?;
            let mut buf = String::new();
            f.read_to_string(&mut buf)?;
            *offset = len;

            for line in buf.lines().filter(|line| include_line(line, min_level)) {
                println!("{line}");
            }
            std::io::stdout().flush()?;
        }
    }
}

fn current_len(path: &Path) -> u64 {
    path.metadata().map(|m| m.len()).unwrap_or(0)
}

fn print_tail(path: &Path, n: usize, min_level: Option<LogLevel>) -> Result<()> {
    // Cheap tail: read the file, split on newlines, take the last n.
    // Berth logs are small (info-level only, no PTY bytes are dumped to
    // the global log), so this is fine.
    let mut f = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let len = f.metadata()?.len() as i64;
    // Cap at ~512 KiB so a huge supervisor log doesn't load entirely.
    let cap = 512 * 1024_i64;
    let offset = std::cmp::max(0, len - cap);
    f.seek(SeekFrom::Start(offset as u64))?;
    let mut buf = String::new();
    f.read_to_string(&mut buf)?;
    let lines: Vec<&str> = buf.lines().collect();
    let start = lines.len().saturating_sub(n);
    for line in lines[start..]
        .iter()
        .copied()
        .filter(|line| include_line(line, min_level))
    {
        println!("{line}");
    }
    Ok(())
}

fn include_line(line: &str, min_level: Option<LogLevel>) -> bool {
    match min_level {
        Some(min_level) => line_level(line).is_some_and(|level| level >= min_level),
        None => true,
    }
}

fn line_level(line: &str) -> Option<LogLevel> {
    line.split_whitespace()
        .find_map(|token| LogLevel::parse(token.trim_matches(|c: char| !c.is_ascii_alphabetic())))
}

#[cfg(test)]
mod tests {
    use super::{include_line, remote_logs_command, LogLevel};

    #[test]
    fn level_filter_keeps_minimum_and_above() {
        assert!(include_line(
            "2026-05-27T09:00:00Z  WARN berth: reconnecting",
            Some(LogLevel::Warn)
        ));
        assert!(include_line(
            "2026-05-27T09:00:00Z ERROR berth: failed",
            Some(LogLevel::Warn)
        ));
        assert!(!include_line(
            "2026-05-27T09:00:00Z  INFO berth: ok",
            Some(LogLevel::Warn)
        ));
        assert!(!include_line("plain PTY output", Some(LogLevel::Warn)));
    }

    #[test]
    fn remote_logs_command_runs_remote_berth_logs() {
        let command = remote_logs_command(40, Some("warn"), true);
        assert!(command.contains("exec \"$berth_bin\" logs -n 40 --sessions --level 'warn'"));
        assert!(command.contains("BERTH_ATTACH_LOCAL=1"));
    }
}
