use anyhow::{Context, Result};
use berth::session::{self, supervisor};
use portable_pty::PtySize;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub async fn run(workspace: String, supervisor_mode: bool, command: Vec<String>) -> Result<i32> {
    if supervisor_mode {
        return run_supervisor(workspace, command).await;
    }
    run_client(workspace, command).await
}

async fn run_supervisor(workspace: String, command: Vec<String>) -> Result<i32> {
    supervisor::detach_from_terminal().ok();
    let socket_path = session::session_socket(&workspace)?;
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

async fn run_client(workspace: String, command: Vec<String>) -> Result<i32> {
    let socket_path = session::session_socket(&workspace)?;
    if !socket_path.exists() {
        spawn_supervisor(&workspace, &command)?;
        wait_for_socket(&socket_path, Duration::from_secs(5))
            .with_context(|| format!("supervisor never created {}", socket_path.display()))?;
    }
    session::client::attach(&socket_path).await
}

fn spawn_supervisor(workspace: &str, command: &[String]) -> Result<()> {
    let exe = std::env::current_exe().context("locating berth binary")?;
    let mut cmd = Command::new(exe);
    cmd.arg("attach")
        .arg("--supervisor")
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

fn wait_for_socket(socket_path: &std::path::Path, timeout: Duration) -> Result<()> {
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
