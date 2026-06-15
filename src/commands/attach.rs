use crate::commands::reconnect::{ReconnectBackoff, ReconnectWake};
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
    pub force: bool,
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
    match run_inner(workspace, opts).await {
        Err(err) if err.is::<session::client::SessionBusy>() => {
            if let Some(code) = configured_busy_exit_code()? {
                eprintln!("{err}");
                Ok(code)
            } else {
                Err(err)
            }
        }
        result => result,
    }
}

async fn run_inner(workspace: String, opts: AttachOptions) -> Result<i32> {
    if let Some(id) = &opts.session {
        berth::validate_session_id(id)?;
    }
    tracing::info!(
        workspace = %workspace,
        supervisor = opts.supervisor,
        new = opts.new,
        force = opts.force,
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
            Some(id) => start_or_attach_session(workspace, id, opts.command, opts.force).await,
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
    resume(workspace, opts.session, opts.force).await
}

fn configured_busy_exit_code() -> Result<Option<i32>> {
    let Some(raw) = std::env::var_os("BERTH_ATTACH_BUSY_EXIT") else {
        return Ok(None);
    };
    let raw = raw.to_string_lossy();
    let code = raw
        .parse::<i32>()
        .with_context(|| format!("parsing BERTH_ATTACH_BUSY_EXIT={raw:?}"))?;
    Ok(Some(code))
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
    let stable_session_target = session.is_some() && !opts.list;
    let mut saw_transport_loss = false;
    let mut reconnect_backoff = ReconnectBackoff::from_env();
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
            opts.force,
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

        let retrying_transport = code == 255 && stable_session_target;
        let retrying_busy =
            code == session::SESSION_BUSY_EXIT && stable_session_target && saw_transport_loss;
        if !retrying_transport && !retrying_busy {
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
                } else if code == session::SESSION_BUSY_EXIT {
                    "session-busy"
                } else {
                    "remote-command-exit"
                },
                "remote attach loop finished"
            );
            return Ok(code);
        }

        if retrying_transport && attempt == 1 && !remote_probe_succeeded {
            tracing::warn!(
                workspace,
                host,
                session_id = session_label,
                attempt,
                "remote host was not reachable during preflight; not entering attach reconnect loop"
            );
            return Ok(code);
        }

        let backoff_ms = reconnect_backoff.current_ms();
        if retrying_transport {
            saw_transport_loss = true;
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
                    "· connection lost; reconnecting remote session {session_label}...  (press any key to retry now, Ctrl+C to abort)"
                );
            } else if attempt.is_multiple_of(4) {
                eprintln!(
                    "· still reconnecting remote session {session_label} (attempt {attempt})...  (press any key to retry now)"
                );
            }
        } else {
            tracing::warn!(
                workspace,
                host,
                session_id = session_label,
                attempt,
                backoff_ms,
                "remote attach session is still attached elsewhere after transport loss; retrying"
            );
            eprintln!(
                "· remote session {session_label} still attached elsewhere; retrying...  (press any key to retry now, Ctrl+C to abort)"
            );
        }

        if matches!(
            reconnect_backoff.wait_and_advance().await?,
            ReconnectWake::KeyPressed
        ) {
            tracing::info!(
                workspace,
                host,
                session_id = session_label,
                attempt,
                retrying_transport,
                retrying_busy,
                "remote attach reconnect wait interrupted by keypress"
            );
        }
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
    let workdir = supervisor_workdir(&workspace);
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

fn supervisor_workdir(workspace: &str) -> Option<PathBuf> {
    std::env::var_os("BERTH_SUPERVISOR_CWD")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| workspace_path(workspace))
}

async fn start_fresh(workspace: String, command: Vec<String>) -> Result<i32> {
    let id = session::new_session_id();
    tracing::info!(
        workspace = %workspace,
        session_id = %id,
        "creating fresh session id"
    );
    start_or_attach_session(workspace, id, command, false).await
}

async fn start_or_attach_session(
    workspace: String,
    id: String,
    command: Vec<String>,
    force: bool,
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
        return attach_resuming_session(&workspace, &id, &socket_path, force).await;
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

async fn resume(workspace: String, session: Option<String>, force: bool) -> Result<i32> {
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
                // The exact session the caller asked for is gone. This is a
                // routine, recoverable condition for the `berth enter`
                // reconnect loop (the supervisor died while we were away),
                // so report it with a dedicated exit code rather than a
                // generic failure. The loop translates this into "mint a
                // fresh session" instead of retrying a dead id forever.
                eprintln!(
                    "no session '{id}' for workspace '{workspace}' (have: {})",
                    if sessions.is_empty() {
                        "none".to_string()
                    } else {
                        sessions.join(", ")
                    }
                );
                tracing::warn!(
                    workspace = %workspace,
                    session_id = %id,
                    available = sessions.len(),
                    exit_code = session::SESSION_NOT_FOUND_EXIT,
                    "requested session not found; returning session-not-found exit code"
                );
                return Ok(session::SESSION_NOT_FOUND_EXIT);
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
            exit_code = session::SESSION_NOT_FOUND_EXIT,
            "resume target socket missing"
        );
        // The id was listed but its socket vanished between the scan and
        // now — the supervisor exited under us. Same recoverable case as a
        // missing id: hand the reconnect loop the session-not-found code.
        eprintln!(
            "session socket '{}' missing; the supervisor may have just exited",
            socket_path.display()
        );
        return Ok(session::SESSION_NOT_FOUND_EXIT);
    }
    tracing::info!(
        workspace = %workspace,
        session_id = %target,
        socket = %socket_path.display(),
        "resume attach found existing socket"
    );
    attach_resuming_session(&workspace, &target, &socket_path, force).await
}

async fn attach_resuming_session(
    workspace: &str,
    session_id: &str,
    socket_path: &Path,
    force: bool,
) -> Result<i32> {
    match session::client::attach(socket_path).await {
        Err(err)
            if err.is::<session::client::SessionBusy>()
                && (force || std::env::var_os("BERTH_ATTACH_TAKEOVER").is_some()) =>
        {
            let mode = if force {
                AttachReclaimMode::Force
            } else {
                AttachReclaimMode::StaleOnly
            };
            if reclaim_attach_owner(workspace, session_id, socket_path, mode)? {
                return session::client::attach(socket_path).await;
            }
            Err(err)
        }
        result => result,
    }
}

#[derive(Clone, Copy)]
enum AttachReclaimMode {
    StaleOnly,
    Force,
}

fn reclaim_attach_owner(
    workspace: &str,
    session_id: &str,
    socket_path: &Path,
    mode: AttachReclaimMode,
) -> Result<bool> {
    #[cfg(not(unix))]
    {
        let _ = (workspace, session_id, socket_path, mode);
        Ok(false)
    }
    #[cfg(unix)]
    {
        use nix::errno::Errno;
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;

        let lock_path = session::client::session_lock_path(socket_path);
        let Some(pid) = read_attach_lock_pid(&lock_path)? else {
            tracing::warn!(
                workspace,
                session_id,
                lock = %lock_path.display(),
                "cannot reclaim busy session because lock owner pid is missing"
            );
            return Ok(false);
        };
        let matching_owner = match mode {
            AttachReclaimMode::StaleOnly => stale_attach_owner_matches(pid, workspace, session_id)?,
            AttachReclaimMode::Force => attach_owner_matches(pid, workspace, session_id)?,
        };
        if !matching_owner {
            tracing::warn!(
                workspace,
                session_id,
                pid,
                force = matches!(mode, AttachReclaimMode::Force),
                "busy session owner is not a matching reclaimable attach process; not reclaiming"
            );
            return Ok(false);
        }

        match mode {
            AttachReclaimMode::StaleOnly => {
                tracing::warn!(
                    workspace,
                    session_id,
                    pid,
                    "reclaiming stale attach client for reconnect"
                );
                eprintln!(
                    "session {session_id} is still held by stale attach pid {pid}; reclaiming..."
                );
            }
            AttachReclaimMode::Force => {
                tracing::warn!(
                    workspace,
                    session_id,
                    pid,
                    "force-detaching existing attach client"
                );
                eprintln!("session {session_id} is attached by pid {pid}; force-detaching it...");
            }
        }

        match kill(Pid::from_raw(pid), Signal::SIGTERM) {
            Ok(()) | Err(Errno::ESRCH) => {}
            Err(err) => return Err(anyhow::Error::new(err).context("terminating stale attach")),
        }
        if wait_until_session_free(socket_path, Duration::from_secs(2)) {
            return Ok(true);
        }

        match kill(Pid::from_raw(pid), Signal::SIGKILL) {
            Ok(()) | Err(Errno::ESRCH) => {}
            Err(err) => return Err(anyhow::Error::new(err).context("killing stale attach")),
        }
        Ok(wait_until_session_free(socket_path, Duration::from_secs(1)))
    }
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

    // The supervisor's own argv (PID-1 of the session, give or take the
    // wrapper below). Built once so both the systemd-managed and the plain
    // detached spawn run exactly the same thing.
    let mut inner: Vec<String> = vec![
        "attach".into(),
        "--supervisor".into(),
        "--session".into(),
        session_id.into(),
        workspace.into(),
    ];
    if !command.is_empty() {
        inner.push("--".into());
        inner.extend(command.iter().cloned());
    }

    // Prefer launching under the per-user systemd manager. A bare detached
    // child lives in the SSH login session's `session-NNN.scope`, which
    // systemd reaps the instant that session ends (KillUserProcesses=yes is
    // the default) — so the session would vanish the moment the user closes
    // the laptop. `systemd-run --user --scope` moves the supervisor into
    // `user@.service` instead, and enabling linger keeps that manager alive
    // across logout, so the session survives until it's idle-reaped or the
    // box reboots. Falls back to a plain detached spawn where there is no
    // usable user manager (no systemd, no user bus, macOS, containers).
    if let Some(systemd_run) = supervisor_systemd_run() {
        best_effort_enable_linger();
        match spawn_supervisor_via_systemd(
            &systemd_run,
            &exe,
            workspace,
            session_id,
            &inner,
            &log_path,
        ) {
            Ok(()) => return Ok(()),
            Err(err) => {
                tracing::warn!(
                    workspace,
                    session_id,
                    error = %format!("{err:#}"),
                    "systemd-run supervisor launch failed; falling back to a plain detached spawn"
                );
            }
        }
    }

    spawn_supervisor_detached(&exe, workspace, session_id, &inner, &log_path)
}

/// Open the supervisor logfile and return two clones (stdout + stderr).
fn open_supervisor_log(log_path: &Path) -> Result<(std::fs::File, std::fs::File)> {
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .with_context(|| format!("opening supervisor log {}", log_path.display()))?;
    let log_clone = log
        .try_clone()
        .with_context(|| "duplicating supervisor log fd")?;
    Ok((log, log_clone))
}

/// Plain detached spawn — the original behaviour. Used directly when no user
/// systemd manager is available, and as the fallback if `systemd-run` fails.
fn spawn_supervisor_detached(
    exe: &Path,
    workspace: &str,
    session_id: &str,
    inner: &[String],
    log_path: &Path,
) -> Result<()> {
    let (log, log_clone) = open_supervisor_log(log_path)?;
    let mut cmd = Command::new(exe);
    cmd.args(inner)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_clone))
        .stderr(Stdio::from(log));
    if let Some(cwd) = std::env::var_os("BERTH_SUPERVISOR_CWD") {
        cmd.env("BERTH_SUPERVISOR_CWD", cwd);
    }
    tracing::info!(
        workspace,
        session_id,
        exe = %exe.display(),
        log = %log_path.display(),
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

/// Launch the supervisor inside a transient `--user --scope` unit so it lives
/// under `user@.service` and survives the SSH session that created it.
fn spawn_supervisor_via_systemd(
    systemd_run: &Path,
    exe: &Path,
    workspace: &str,
    session_id: &str,
    inner: &[String],
    log_path: &Path,
) -> Result<()> {
    let (log, log_clone) = open_supervisor_log(log_path)?;
    let unit = format!("berth-{}-{}", sanitize_unit(workspace), session_id);
    let mut cmd = Command::new(systemd_run);
    cmd.arg("--user")
        .arg("--scope")
        .arg("--quiet")
        // Garbage-collect the transient unit when the supervisor exits so a
        // dead session never leaves a lingering failed unit behind.
        .arg("--collect")
        .arg(format!("--unit={unit}"));
    if let Some(cwd) = std::env::var_os("BERTH_SUPERVISOR_CWD") {
        cmd.arg(format!(
            "--setenv=BERTH_SUPERVISOR_CWD={}",
            cwd.to_string_lossy()
        ));
    }
    cmd.arg("--")
        .arg(exe)
        .args(inner)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_clone))
        .stderr(Stdio::from(log));
    tracing::info!(
        workspace,
        session_id,
        unit = %unit,
        log = %log_path.display(),
        "launching session supervisor under user@.service via systemd-run"
    );
    let mut child = cmd.spawn().context("spawning systemd-run supervisor")?;

    // `systemd-run --scope` stays alive for the supervisor's lifetime, so a
    // healthy launch is still running after a short grace period. If it has
    // already exited, the user bus refused us (or the unit name clashed) —
    // surface that to the caller so it can fall back to a plain spawn rather
    // than leaving the user with a session that never appears.
    std::thread::sleep(Duration::from_millis(250));
    match child.try_wait().context("polling systemd-run")? {
        Some(status) if !status.success() => {
            anyhow::bail!("systemd-run exited early with {status}");
        }
        _ => {
            tracing::info!(
                workspace,
                session_id,
                unit = %unit,
                "session supervisor scope is live under user@.service"
            );
            Ok(())
        }
    }
}

/// systemd unit names allow `[A-Za-z0-9:_.\-]`; map everything else (notably
/// the `/` in `org/project` workspaces) to `-` so the generated `--unit` name
/// is always valid.
fn sanitize_unit(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '.' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Locate `systemd-run` only when a per-user systemd manager is actually
/// reachable — the presence of the user D-Bus socket at
/// `$XDG_RUNTIME_DIR/bus` is the reliable signal that `systemd-run --user`
/// will succeed. Linux-only; everywhere else this returns None and callers
/// take the plain-spawn path.
#[cfg(target_os = "linux")]
fn supervisor_systemd_run() -> Option<PathBuf> {
    if std::env::var_os("BERTH_NO_SYSTEMD_SCOPE").is_some() {
        return None;
    }
    let xdg = std::env::var_os("XDG_RUNTIME_DIR")?;
    if !Path::new(&xdg).join("bus").exists() {
        return None;
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("systemd-run"))
        .find(|candidate| candidate.is_file())
}

#[cfg(not(target_os = "linux"))]
fn supervisor_systemd_run() -> Option<PathBuf> {
    None
}

/// Best-effort `loginctl enable-linger` for the current user so the user
/// systemd manager (and the supervisor scopes under it) keep running after
/// the last login session ends. Self-linger is permitted without privilege
/// on stock systemd via polkit; if it is denied we just proceed — the
/// supervisor still survives an SSH drop, only a full logout would reap it.
#[cfg(target_os = "linux")]
fn best_effort_enable_linger() {
    let result = Command::new("loginctl")
        .arg("enable-linger")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match result {
        Ok(status) if status.success() => {
            tracing::debug!("loginctl enable-linger ok");
        }
        Ok(status) => {
            tracing::debug!(%status, "loginctl enable-linger denied; continuing without linger");
        }
        Err(err) => {
            tracing::debug!(error = %err, "loginctl not available; continuing without linger");
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn best_effort_enable_linger() {}

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

#[cfg(unix)]
fn read_attach_lock_pid(lock_path: &Path) -> Result<Option<i32>> {
    let content = match std::fs::read_to_string(lock_path) {
        Ok(content) => content,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("reading {}", lock_path.display())),
    };
    Ok(content.lines().find_map(|line| {
        line.strip_prefix("pid=")
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|pid| pid.parse::<i32>().ok())
    }))
}

#[cfg(unix)]
fn wait_until_session_free(socket_path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if session::client::is_session_free(socket_path) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    session::client::is_session_free(socket_path)
}

#[cfg(unix)]
fn stale_attach_owner_matches(pid: i32, workspace: &str, session_id: &str) -> Result<bool> {
    if !attach_owner_matches(pid, workspace, session_id)? {
        return Ok(false);
    }
    let Some(ppid) = proc_ppid(pid)? else {
        return Ok(false);
    };
    Ok(ppid <= 1 || !Path::new("/proc").join(ppid.to_string()).exists())
}

#[cfg(unix)]
fn attach_owner_matches(pid: i32, workspace: &str, session_id: &str) -> Result<bool> {
    let args = proc_cmdline_args(pid)?;
    Ok(attach_owner_cmdline_matches(&args, workspace, session_id))
}

#[cfg(unix)]
fn proc_cmdline_args(pid: i32) -> Result<Vec<String>> {
    let path = Path::new("/proc").join(pid.to_string()).join("cmdline");
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    Ok(bytes
        .split(|b| *b == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).to_string())
        .collect())
}

#[cfg(unix)]
fn proc_ppid(pid: i32) -> Result<Option<i32>> {
    let path = Path::new("/proc").join(pid.to_string()).join("status");
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    Ok(content.lines().find_map(|line| {
        line.strip_prefix("PPid:")
            .and_then(|rest| rest.trim().parse::<i32>().ok())
    }))
}

#[cfg(any(unix, test))]
fn attach_owner_cmdline_matches(args: &[String], workspace: &str, session_id: &str) -> bool {
    if args.iter().any(|arg| arg == "--supervisor") {
        return false;
    }
    let Some(attach_idx) = args.iter().position(|arg| arg == "attach") else {
        return false;
    };
    let Some(session_flag_idx) = args.iter().position(|arg| arg == "--session") else {
        return false;
    };
    if session_flag_idx <= attach_idx {
        return false;
    }
    if args.get(session_flag_idx + 1).map(String::as_str) != Some(session_id) {
        return false;
    }
    args.iter().skip(attach_idx + 1).any(|arg| arg == workspace)
}

#[cfg(test)]
mod tests {
    use super::{attach_owner_cmdline_matches, sanitize_unit, supervisor_workdir};
    use std::path::PathBuf;

    #[test]
    fn sanitize_unit_maps_slash_and_keeps_valid_chars() {
        // `/` (org/project) and other separators become `-`; the systemd-safe
        // set [A-Za-z0-9:_.-] passes through untouched.
        assert_eq!(sanitize_unit("atlas/Atlas"), "atlas-Atlas");
        assert_eq!(sanitize_unit("team/proj-1"), "team-proj-1");
        assert_eq!(sanitize_unit("a.b_c-d:e"), "a.b_c-d:e");
        // `.` is a valid unit-name char, so only the `/` is rewritten.
        assert_eq!(sanitize_unit("../etc"), "..-etc");
        // The result is always a single token a `--unit=` value can hold.
        assert!(sanitize_unit("x y\tz").chars().all(|c| !c.is_whitespace()));
    }

    #[test]
    fn supervisor_workdir_prefers_explicit_remote_enter_cwd() {
        let prior = std::env::var_os("BERTH_SUPERVISOR_CWD");
        std::env::set_var("BERTH_SUPERVISOR_CWD", "/home/ubuntu/Projects/postil-dev");
        assert_eq!(
            supervisor_workdir("postil/dev"),
            Some(PathBuf::from("/home/ubuntu/Projects/postil-dev"))
        );
        match prior {
            Some(value) => std::env::set_var("BERTH_SUPERVISOR_CWD", value),
            None => std::env::remove_var("BERTH_SUPERVISOR_CWD"),
        }
    }

    #[test]
    fn attach_owner_cmdline_matches_same_workspace_and_session_only() {
        let owner = vec![
            "/home/ubuntu/.local/bin/berth".to_string(),
            "attach".to_string(),
            "--new".to_string(),
            "--session".to_string(),
            "1b02652d9683".to_string(),
            "postil/dev".to_string(),
        ];
        assert!(attach_owner_cmdline_matches(
            &owner,
            "postil/dev",
            "1b02652d9683"
        ));
        assert!(!attach_owner_cmdline_matches(
            &owner,
            "other/dev",
            "1b02652d9683"
        ));
        assert!(!attach_owner_cmdline_matches(
            &owner,
            "postil/dev",
            "different"
        ));

        let supervisor = vec![
            "berth".to_string(),
            "attach".to_string(),
            "--supervisor".to_string(),
            "--session".to_string(),
            "1b02652d9683".to_string(),
            "postil/dev".to_string(),
        ];
        assert!(!attach_owner_cmdline_matches(
            &supervisor,
            "postil/dev",
            "1b02652d9683"
        ));
    }
}
