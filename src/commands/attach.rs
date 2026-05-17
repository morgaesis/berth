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

async fn run_supervisor(workspace: String, session_id: String, command: Vec<String>) -> Result<i32> {
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
    spawn_supervisor(&workspace, &id, &command)?;
    wait_for_socket(&socket_path, Duration::from_secs(5))
        .with_context(|| format!("supervisor never created {}", socket_path.display()))?;
    session::client::attach(&socket_path).await
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

fn spawn_supervisor(workspace: &str, session_id: &str, command: &[String]) -> Result<()> {
    let exe = std::env::current_exe().context("locating berth binary")?;
    let mut cmd = Command::new(exe);
    cmd.arg("attach")
        .arg("--supervisor")
        .arg("--session")
        .arg(session_id)
        .arg(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
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
