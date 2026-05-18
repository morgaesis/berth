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
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const DEFAULT_LINES: usize = 200;

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

pub async fn run(lines: Option<usize>, follow: bool, sessions: bool) -> Result<()> {
    let n = lines.unwrap_or(DEFAULT_LINES);

    if let Some(path) = global_log_path() {
        if path.exists() {
            println!("=== {} (last {n} lines) ===", path.display());
            print_tail(&path, n)?;
        } else {
            println!("(no global log at {})", path.display());
        }
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
                    print_tail(&log, n)?;
                }
            }
        }
    }

    if follow {
        println!(
            "\n--follow not implemented yet; use `tail -F {}`",
            global_log_path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "<log>".into())
        );
    }
    Ok(())
}

fn print_tail(path: &Path, n: usize) -> Result<()> {
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
    for line in &lines[start..] {
        println!("{line}");
    }
    Ok(())
}
