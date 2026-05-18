use anyhow::{bail, Context, Result};
use berth::session::{self, supervisor};
use portable_pty::PtySize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct AttachOptions {
    pub supervisor: bool,
    pub new: bool,
    pub session: Option<String>,
    pub list: bool,
    pub command: Vec<String>,
}

pub async fn run(workspace: String, opts: AttachOptions) -> Result<i32> {
    if let Some(id) = &opts.session {
        berth::validate_session_id(id)?;
    }
    if opts.supervisor {
        let id = opts
            .session
            .clone()
            .context("--supervisor requires --session <id>")?;
        return run_supervisor(workspace, id, opts.command).await;
    }
    if opts.list {
        if !opts.command.is_empty() {
            bail!("--list does not accept a command override");
        }
        return list_sessions(&workspace);
    }
    if opts.new {
        return start_fresh(workspace, opts.command).await;
    }
    if !opts.command.is_empty() {
        bail!(
            "command override is only valid with --new (resuming an existing session inherits its original command)"
        );
    }
    resume(workspace, opts.session).await
}

async fn run_supervisor(
    workspace: String,
    session_id: String,
    command: Vec<String>,
) -> Result<i32> {
    supervisor::detach_from_terminal().ok();
    let socket_path = session::session_socket(&workspace, &session_id)?;
    let workdir = workspace_path(&workspace);
    let cfg = supervisor::SupervisorConfig {
        socket_path,
        workspace,
        command,
        workdir,
        initial_size: PtySize {
            cols: 100,
            rows: 30,
            pixel_width: 0,
            pixel_height: 0,
        },
    };
    supervisor::run(cfg).await
}

async fn start_fresh(workspace: String, command: Vec<String>) -> Result<i32> {
    let id = session::new_session_id();
    let sessions_dir = session::sessions_dir(&workspace)?;
    std::fs::create_dir_all(&sessions_dir)
        .with_context(|| format!("creating sessions dir {}", sessions_dir.display()))?;
    let socket_path = session::session_socket(&workspace, &id)?;
    let log_path = supervisor_log_path(&workspace, &id)?;
    spawn_supervisor(&workspace, &id, &command)?;
    if let Err(_e) = wait_for_socket(&socket_path, Duration::from_secs(5)) {
        // Supervisor failed to start or exited before the socket was
        // ready. Keep the visible error short and direct; the full
        // detail (tracing + child stdout/stderr) is in `berth logs`.
        use colored::Colorize;
        let mut hint = command_failure_hint(&command);
        if !hint.is_empty() {
            hint = format!("  tip: {}\n", hint.yellow());
        }
        anyhow::bail!(
            "{}: supervisor for '{}' exited before connecting (likely the command failed or finished immediately)\n\
             {hint}  details: `{}` (or read {})",
            "✗ berth".red().bold(),
            workspace,
            "berth logs".cyan(),
            log_path.display().to_string().dimmed(),
        );
    }
    session::client::attach(&socket_path).await
}

/// Short, single-line hint based on the command shape. Empty when we
/// have nothing useful to say.
fn command_failure_hint(command: &[String]) -> String {
    if command.is_empty() {
        return String::new();
    }
    let first = command[0].as_str();
    // Single-token shell wrappers (bash/sh/zsh/dash) — the user already
    // wrapped, so don't recursively suggest wrapping again.
    let is_shell_wrapper = matches!(first, "bash" | "sh" | "zsh" | "dash" | "ash");
    if is_shell_wrapper {
        return String::new();
    }
    if command.len() == 1 && first.contains(char::is_whitespace) {
        // Whole thing was passed as one quoted arg.
        return format!(
            "`{first}` was treated as one binary path; for shell parsing use `-- bash -lc '<cmd>'`"
        );
    }
    format!(
        "for shell aliases or login profile, wrap: `-- bash -lc '{}'`",
        command.join(" ")
    )
}

async fn resume(workspace: String, session: Option<String>) -> Result<i32> {
    let sessions = session::list_sessions(&workspace)?;
    let target = match session {
        Some(id) => {
            if !sessions.iter().any(|s| s == &id) {
                bail!(
                    "no session '{id}' for workspace '{workspace}' (have: {})",
                    if sessions.is_empty() {
                        "none".to_string()
                    } else {
                        sessions.join(", ")
                    }
                );
            }
            id
        }
        None => match sessions.as_slice() {
            [] => bail!(
                "no resumable session for workspace '{workspace}'; start one with `berth enter {workspace}` or `berth attach --new {workspace}`"
            ),
            [only] => only.clone(),
            many => bail!(
                "multiple sessions for workspace '{workspace}': {}\n  pick one with `berth attach --session <id> {workspace}`",
                many.join(", ")
            ),
        },
    };
    let socket_path = session::session_socket(&workspace, &target)?;
    if !socket_path.exists() {
        bail!(
            "session socket '{}' missing; the supervisor may have just exited",
            socket_path.display()
        );
    }
    session::client::attach(&socket_path).await
}

fn list_sessions(workspace: &str) -> Result<i32> {
    let sessions = session::list_sessions(workspace)?;
    if sessions.is_empty() {
        println!("(no sessions for workspace '{workspace}')");
    } else {
        for id in sessions {
            println!("{id}");
        }
    }
    Ok(0)
}

/// Where the supervisor for `(workspace, session_id)` redirects its
/// stdout+stderr. Stored alongside the socket file under sessions_dir
/// so `berth logs` (and ad-hoc debugging) can find it easily.
pub fn supervisor_log_path(workspace: &str, session_id: &str) -> Result<std::path::PathBuf> {
    let dir = berth::session::sessions_dir(workspace)?;
    Ok(dir.join(format!("{session_id}.log")))
}

fn spawn_supervisor(workspace: &str, session_id: &str, command: &[String]) -> Result<()> {
    let exe = std::env::current_exe().context("locating berth binary")?;
    let log_path = supervisor_log_path(workspace, session_id)?;
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating supervisor log dir {}", parent.display()))?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("opening supervisor log {}", log_path.display()))?;
    let log_clone = log
        .try_clone()
        .with_context(|| "duplicating supervisor log fd")?;
    let mut cmd = Command::new(exe);
    cmd.arg("attach")
        .arg("--supervisor")
        .arg("--session")
        .arg(session_id)
        .arg(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_clone))
        .stderr(Stdio::from(log));
    if !command.is_empty() {
        cmd.arg("--");
        for arg in command {
            cmd.arg(arg);
        }
    }
    cmd.spawn().context("spawning session supervisor")?;
    Ok(())
}

fn wait_for_socket(socket_path: &Path, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if socket_path.exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!("timed out waiting for supervisor socket")
}

fn workspace_path(name: &str) -> Option<PathBuf> {
    let projects = dirs::data_dir()?.join("berth").join("projects").join(name);
    projects.exists().then_some(projects)
}
