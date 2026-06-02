use anyhow::{bail, Context, Result};
use berth::config::Config;
use berth::session::{self, supervisor};
use berth::ssh;
use portable_pty::PtySize;
use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct AttachOptions {
    pub supervisor: bool,
    pub new: bool,
    /// Attach to the newest free session, or create one if none are
    /// available. Kept for explicit attach workflows; `berth enter`
    /// uses a generated session id instead.
    pub resume_or_new: bool,
    pub session: Option<String>,
    pub list: bool,
    pub list_all: bool,
    pub list_long: bool,
    pub session_counts: bool,
    pub command: Vec<String>,
}

pub async fn run(workspace: String, opts: AttachOptions) -> Result<i32> {
    if let Some(id) = &opts.session {
        berth::validate_session_id(id)?;
    }
    tracing::info!(
        workspace = %workspace,
        supervisor = opts.supervisor,
        new = opts.new,
        resume_or_new = opts.resume_or_new,
        session_id = opts.session.as_deref().unwrap_or(""),
        list = opts.list,
        list_all = opts.list_all,
        list_long = opts.list_long,
        session_counts = opts.session_counts,
        command_len = opts.command.len(),
        attach_local = std::env::var_os("BERTH_ATTACH_LOCAL").is_some(),
        "attach command starting"
    );
    if !opts.supervisor {
        if let Some(code) = maybe_remote_attach(&workspace, &opts).await? {
            return Ok(code);
        }
    }
    if opts.supervisor {
        let id = opts
            .session
            .clone()
            .context("--supervisor requires --session <id>")?;
        return run_supervisor(workspace, id, opts.command).await;
    }
    if opts.session_counts {
        let (live, attached, exited) = session_inventory_counts(&workspace)?;
        println!("{live}\t{attached}\t{exited}");
        return Ok(0);
    }
    if opts.list {
        if !opts.command.is_empty() {
            bail!("--list does not accept a command override");
        }
        return list_sessions(&workspace, opts.list_all, opts.list_long);
    }
    if opts.new {
        return match opts.session {
            Some(id) => start_or_attach_session(workspace, id, opts.command).await,
            None => start_fresh(workspace, opts.command).await,
        };
    }
    if opts.resume_or_new {
        return resume_or_new(workspace, opts.command).await;
    }
    if !opts.command.is_empty() {
        bail!(
            "command override is only valid with --new (resuming an existing session inherits its original command)"
        );
    }
    resume(workspace, opts.session).await
}

async fn maybe_remote_attach(workspace: &str, opts: &AttachOptions) -> Result<Option<i32>> {
    if std::env::var_os("BERTH_ATTACH_LOCAL").is_some() {
        tracing::debug!(
            workspace,
            "BERTH_ATTACH_LOCAL set; handling attach on this host"
        );
        return Ok(None);
    }
    let config = Config::load()?;
    let Some(ws) = config.workspaces.get(workspace) else {
        tracing::debug!(workspace, "workspace not in config; using local attach");
        return Ok(None);
    };
    let Some(host) = config.resolved_remote(workspace, ws) else {
        tracing::debug!(
            workspace,
            "workspace has no resolved remote; using local attach"
        );
        return Ok(None);
    };
    tracing::info!(
        workspace,
        host = %host,
        session_id = opts.session.as_deref().unwrap_or(""),
        new = opts.new,
        list = opts.list,
        "delegating attach to remote host"
    );
    let code = remote_attach_with_reconnect(&host, workspace, opts).await?;
    Ok(Some(code))
}

async fn remote_attach_with_reconnect(
    host: &str,
    workspace: &str,
    opts: &AttachOptions,
) -> Result<i32> {
    let generated_session = if opts.new && opts.session.is_none() && !opts.list {
        Some(session::new_session_id())
    } else {
        None
    };
    let session = opts.session.as_deref().or(generated_session.as_deref());
    let session_label = session.unwrap_or("-");
    tracing::info!(
        host,
        workspace,
        requested_session_id = opts.session.as_deref().unwrap_or(""),
        generated_session_id = generated_session.as_deref().unwrap_or(""),
        effective_session_id = session_label,
        new = opts.new,
        list = opts.list,
        session_id_reused_on_reconnect = session.is_some() && !opts.list,
        "remote attach reconnect loop starting"
    );

    let remote_probe_succeeded = if opts.list {
        true
    } else {
        ssh::run_remote_command_with_timeout(host, "true", Duration::from_secs(5))
            .await
            .is_ok()
    };
    let mut backoff_ms: u64 = 500;
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        let code = ssh::ssh_attach_remote(
            host,
            workspace,
            opts.list,
            opts.list_all,
            opts.list_long,
            session,
            opts.new,
            &opts.command,
        )
        .await?;
        tracing::info!(
            code,
            attempt,
            host,
            workspace,
            session_id = session_label,
            reused_session_id = attempt > 1 && session.is_some(),
            "remote attach returned"
        );

        if code != 255 || opts.list || session.is_none() {
            tracing::info!(
                code,
                attempt,
                host,
                workspace,
                session_id = session_label,
                exit_reason = if opts.list {
                    "list-complete"
                } else if session.is_none() {
                    "no-stable-session-target"
                } else {
                    "remote-command-exit"
                },
                "remote attach loop finished"
            );
            return Ok(code);
        }

        if attempt == 1 && !remote_probe_succeeded {
            tracing::warn!(
                workspace,
                host,
                session_id = session_label,
                attempt,
                "remote host was not reachable during preflight; not entering attach reconnect loop"
            );
            return Ok(code);
        }

        tracing::warn!(
            workspace,
            host,
            session_id = session_label,
            attempt,
            backoff_ms,
            reused_session_id_on_next_attempt = session.is_some(),
            "remote attach transport lost; reconnecting"
        );
        if attempt == 1 {
            eprintln!(
                "· connection lost; reconnecting remote session {session_label}...  (Ctrl+C to abort)"
            );
        } else if attempt.is_multiple_of(4) {
            eprintln!("· still reconnecting remote session {session_label} (attempt {attempt})...");
        }
        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        backoff_ms = (backoff_ms.saturating_mul(2)).min(10_000);
    }
}

/// Smart attach: try each live session in turn; the first one whose
/// client-flock is free (i.e. no other client is currently connected
/// — the prior client exited cleanly OR died via SSH-drop /
/// hibernation, releasing the kernel-held lock) is attached. If every
/// live session is busy (another tab is connected), spawn a fresh
/// supervisor instead. If none exist at all, also spawn fresh.
async fn resume_or_new(workspace: String, command: Vec<String>) -> Result<i32> {
    let sessions = session::list_sessions(&workspace)?;
    let mut live: Vec<String> = sessions
        .into_iter()
        .filter(|id| {
            session::session_socket(&workspace, id)
                .map(|p| p.exists())
                .unwrap_or(false)
        })
        .collect();
    tracing::info!(
        workspace = %workspace,
        candidate_sessions = live.len(),
        "resume-or-new scanning live sessions"
    );
    live.sort_by(|a, b| {
        session_mtime(&workspace, b)
            .cmp(&session_mtime(&workspace, a))
            .then_with(|| a.cmp(b))
    });
    for id in &live {
        let socket = session::session_socket(&workspace, id)?;
        if session::client::is_session_free(&socket) {
            tracing::info!(
                workspace = %workspace,
                session_id = %id,
                socket = %socket.display(),
                "resume-or-new found existing free socket; attaching"
            );
            return session::client::attach(&socket).await;
        }
        tracing::debug!(
            workspace = %workspace,
            session_id = %id,
            socket = %socket.display(),
            "resume-or-new socket is busy"
        );
    }
    // No free session — every existing one has a connected client (a
    // sibling tab is attached). Honor the user's intent to be in this
    // workspace by spawning a new supervisor for them.
    start_fresh(workspace, command).await
}

fn session_mtime(workspace: &str, id: &str) -> Option<std::time::SystemTime> {
    session::session_socket(workspace, id)
        .ok()
        .and_then(|path| session::client::session_activity_time(&path))
}

async fn run_supervisor(
    workspace: String,
    session_id: String,
    command: Vec<String>,
) -> Result<i32> {
    supervisor::detach_from_terminal().ok();
    let socket_path = session::session_socket(&workspace, &session_id)?;
    let workdir = workspace_path(&workspace);
    tracing::info!(
        workspace = %workspace,
        session_id = %session_id,
        socket = %socket_path.display(),
        workdir = workdir.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
        command_len = command.len(),
        "session supervisor starting from attach command"
    );
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
    tracing::info!(
        workspace = %workspace,
        session_id = %id,
        "creating fresh session id"
    );
    start_or_attach_session(workspace, id, command).await
}

async fn start_or_attach_session(
    workspace: String,
    id: String,
    command: Vec<String>,
) -> Result<i32> {
    let sessions_dir = session::sessions_dir(&workspace)?;
    std::fs::create_dir_all(&sessions_dir)
        .with_context(|| format!("creating sessions dir {}", sessions_dir.display()))?;
    let socket_path = session::session_socket(&workspace, &id)?;
    if socket_path.exists() {
        tracing::info!(
            workspace = %workspace,
            session_id = %id,
            socket = %socket_path.display(),
            "existing session socket found; attaching"
        );
        return session::client::attach(&socket_path).await;
    }
    let log_path = supervisor_log_path(&workspace, &id)?;
    tracing::info!(
        workspace = %workspace,
        session_id = %id,
        socket = %socket_path.display(),
        log = %log_path.display(),
        command_len = command.len(),
        "no existing socket; spawning supervisor"
    );
    spawn_supervisor(&workspace, &id, &command)?;
    if wait_for_socket(&socket_path, Duration::from_secs(5)).is_err() {
        tracing::warn!(
            workspace = %workspace,
            session_id = %id,
            socket = %socket_path.display(),
            "supervisor socket did not appear before attach timeout"
        );
        // Keep the visible error to one line. The full detail (child
        // stderr + tracing) is in `berth logs`.
        return Err(command_exited_before_attach_error(&command));
    }
    match session::client::attach(&socket_path).await {
        Ok(code) => {
            tracing::info!(
                workspace = %workspace,
                session_id = %id,
                socket = %socket_path.display(),
                code,
                "attach client exited"
            );
            Ok(code)
        }
        Err(err) if has_io_kind(&err, ErrorKind::ConnectionRefused) => {
            tracing::warn!(
                workspace = %workspace,
                session_id = %id,
                socket = %socket_path.display(),
                error = ?err,
                "attach connection refused; supervisor likely exited before attach"
            );
            Err(command_exited_before_attach_error(&command))
        }
        Err(err) => Err(err),
    }
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
            "`{first}` was treated as one binary path; for shell parsing use `-- bash -ic '<cmd>'`"
        );
    }
    format!(
        "for shell aliases or login profile, wrap: `-- bash -ic '{}'`",
        command.join(" ")
    )
}

fn command_exited_before_attach_error(command: &[String]) -> anyhow::Error {
    use colored::Colorize;
    let hint = command_failure_hint(command);
    let hint_suffix = if hint.is_empty() {
        String::new()
    } else {
        format!(" — {}", hint.dimmed())
    };
    anyhow::anyhow!(
        "{} command exited before berth could attach{hint_suffix}  (`{}` for details)",
        "✗".red().bold(),
        "berth logs".cyan(),
    )
}

async fn resume(workspace: String, session: Option<String>) -> Result<i32> {
    let sessions = session::list_sessions(&workspace)?;
    tracing::info!(
        workspace = %workspace,
        requested_session_id = session.as_deref().unwrap_or(""),
        available_sessions = sessions.len(),
        "resume attach resolving target"
    );
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
        tracing::warn!(
            workspace = %workspace,
            session_id = %target,
            socket = %socket_path.display(),
            "resume target socket missing"
        );
        bail!(
            "session socket '{}' missing; the supervisor may have just exited",
            socket_path.display()
        );
    }
    tracing::info!(
        workspace = %workspace,
        session_id = %target,
        socket = %socket_path.display(),
        "resume attach found existing socket"
    );
    session::client::attach(&socket_path).await
}

fn list_sessions(workspace: &str, all: bool, long: bool) -> Result<i32> {
    if !all && !long {
        let sessions = session::list_sessions(workspace)?;
        if sessions.is_empty() {
            println!(
                "(no active sessions for workspace '{workspace}'; use `berth attach --list --all --long {workspace}` to include exited session logs)"
            );
        } else {
            for id in sessions {
                println!("{id}");
            }
        }
        return Ok(0);
    }

    let sessions = session_inventory(workspace, all)?;
    if sessions.is_empty() {
        if all {
            println!("(no sessions for workspace '{workspace}')");
        } else {
            println!(
                "(no active sessions for workspace '{workspace}'; use `berth attach --list --all --long {workspace}` to include exited session logs)"
            );
        }
    } else {
        println!(
            "{:<14}  {:<7}  {:<8}  {:<3}  UPDATED",
            "SESSION", "STATUS", "ATTACHED", "LOG"
        );
        for s in sessions {
            println!(
                "{:<14}  {:<7}  {:<8}  {:<3}  {}",
                s.id,
                s.status,
                s.attached_label(),
                if s.has_log { "yes" } else { "no" },
                format_epoch(s.updated),
            );
        }
    }
    Ok(0)
}

#[derive(Debug)]
struct SessionRow {
    id: String,
    status: &'static str,
    attached: Option<bool>,
    has_log: bool,
    updated: Option<SystemTime>,
}

impl SessionRow {
    fn attached_label(&self) -> &'static str {
        match self.attached {
            Some(true) => "yes",
            Some(false) => "no",
            None => "-",
        }
    }
}

#[derive(Default)]
struct SessionFiles {
    socket: Option<PathBuf>,
    lock: Option<PathBuf>,
    log: Option<PathBuf>,
}

pub fn session_inventory_counts(workspace: &str) -> Result<(usize, usize, usize)> {
    let rows = session_inventory(workspace, true)?;
    let live = rows.iter().filter(|r| r.status == "live").count();
    let attached = rows.iter().filter(|r| r.attached == Some(true)).count();
    let exited = rows.iter().filter(|r| r.status == "exited").count();
    Ok((live, attached, exited))
}

fn session_inventory(workspace: &str, include_exited: bool) -> Result<Vec<SessionRow>> {
    let dir = session::sessions_dir(workspace)?;
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut by_id: BTreeMap<String, SessionFiles> = BTreeMap::new();
    for entry in std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        let Some(name) = path
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        if let Some(id) = name.strip_suffix(".sock") {
            by_id.entry(id.to_string()).or_default().socket = Some(path);
        } else if let Some(id) = name.strip_suffix(".sock.client-lock") {
            by_id.entry(id.to_string()).or_default().lock = Some(path);
        } else if let Some(id) = name.strip_suffix(".log") {
            by_id.entry(id.to_string()).or_default().log = Some(path);
        }
    }

    let mut rows = Vec::new();
    for (id, files) in by_id {
        let live = files.socket.as_ref().is_some_and(|p| p.exists());
        if !include_exited && !live {
            continue;
        }
        let attached = files
            .socket
            .as_ref()
            .filter(|_| live)
            .and_then(|socket| session::client::is_session_attached(socket));
        rows.push(SessionRow {
            id,
            status: if live { "live" } else { "exited" },
            attached,
            has_log: files.log.is_some(),
            updated: latest_mtime([
                files.socket.as_ref(),
                files.lock.as_ref(),
                files.log.as_ref(),
            ]),
        });
    }
    rows.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));
    Ok(rows)
}

fn latest_mtime<'a>(paths: impl IntoIterator<Item = Option<&'a PathBuf>>) -> Option<SystemTime> {
    paths
        .into_iter()
        .flatten()
        .filter_map(|path| path.metadata().ok()?.modified().ok())
        .max()
}

fn format_epoch(t: Option<SystemTime>) -> String {
    t.and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|| "-".to_string())
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
    let mut cmd = Command::new(&exe);
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
    tracing::info!(
        workspace,
        session_id,
        exe = %exe.display(),
        log = %log_path.display(),
        command_len = command.len(),
        "spawning detached session supervisor"
    );
    let child = cmd.spawn().context("spawning session supervisor")?;
    tracing::info!(
        workspace,
        session_id,
        pid = child.id(),
        "detached session supervisor spawned"
    );
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

fn has_io_kind(err: &anyhow::Error, kind: ErrorKind) -> bool {
    err.chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .any(|io| io.kind() == kind)
}

fn workspace_path(name: &str) -> Option<PathBuf> {
    let projects = dirs::data_dir()?.join("berth").join("projects").join(name);
    projects.exists().then_some(projects)
}
