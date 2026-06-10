use anyhow::Result;
use berth::config::{Config, Runtime, Workspace};
use berth::deploy::{self, ConsentMode, DeployDecision};
use berth::runtime::{self, CommandSpec};
use berth::ssh::{self, RemoteSessionMode};
use colored::Colorize;
use std::env;
use std::fs;
#[cfg(unix)]
use std::io::Read;
use std::io::{self, IsTerminal, Write};
#[cfg(unix)]
use std::os::fd::AsFd;
use std::path::Path;
use std::time::Duration;

/// User-controllable knobs for `berth enter`.
#[derive(Debug, Clone, Default)]
pub struct EnterOptions {
    /// `--plain` / `--no-resume`: skip all session-mux machinery.
    pub plain: bool,
    /// `--auto-deploy`: push the berth binary without prompting.
    pub auto_deploy: bool,
    /// `--no-deploy`: never push; fall through to legacy mux or fail.
    pub no_deploy: bool,
    /// `--no-reconnect`: when SSH exits with status 255 (network
    /// dropped), bail instead of automatically retrying. Default is
    /// to silently reconnect until the network comes back or the user
    /// Ctrl+Cs.
    pub no_reconnect: bool,
    /// `--dir`: override the remote working directory for this run.
    pub dir: Option<String>,
    /// Trailing `-- <argv>`: override the workspace default command.
    pub command: Vec<String>,
}

fn default_projects_path() -> std::path::PathBuf {
    if let Ok(dir) = env::var("BERTH_DATA_DIR") {
        return std::path::PathBuf::from(dir).join("projects");
    }
    if let Ok(dir) = env::var("XDG_DATA_HOME") {
        return std::path::PathBuf::from(dir).join("berth").join("projects");
    }

    dirs::data_local_dir()
        .map(|p| p.join("berth").join("projects"))
        .unwrap_or_else(|| std::path::PathBuf::from("~/.local/share/berth/projects"))
}

pub async fn run(
    name: String,
    remote_override: Option<String>,
    ports_override: Vec<u16>,
    mut opts: EnterOptions,
) -> Result<()> {
    let mut config = Config::load()?;

    // Hook-driven entries set BERTH_FROM_HOOK=1. BERTH_SKIP_AUTO remains
    // the recursion/opt-out guard and is intentionally not treated as a
    // hook marker by itself.
    let from_new_tab_hook = env::var_os("BERTH_FROM_HOOK").is_some();
    if from_new_tab_hook {
        if !config.defaults.new_tab_auto_entry {
            tracing::debug!("new_tab_auto_entry disabled; skipping hook-driven entry");
            return Ok(());
        }
        // Hook-triggered entry is an opportunistic new-tab convenience,
        // not an explicit user command. It must not sit in a reconnect
        // loop, deploy prompt, or SSH key prompt after a network flap.
        opts.no_reconnect = true;
        opts.no_deploy = true;
        maybe_show_new_tab_hint(&name);
    }

    let workspace = if let Some(ws) = config.workspaces.get(&name) {
        ws.clone()
    } else {
        let default_path = default_projects_path().join(&name);

        let path_str = default_path.to_string_lossy().to_string();

        if !default_path.exists() {
            fs::create_dir_all(&default_path)?;
            println!("Created directory: {}", path_str);
        }

        let mut workspace = Workspace::new(path_str.clone());
        workspace.remote = remote_override.clone();
        workspace.ports = if ports_override.is_empty() {
            None
        } else {
            Some(ports_override.clone())
        };

        config.workspaces.insert(name.clone(), workspace.clone());
        config.save()?;
        println!("Created workspace '{}' at {}", name, path_str);

        workspace
    };

    let path = Path::new(&workspace.path);
    if !path.exists() {
        fs::create_dir_all(path)?;
    }

    // Resolve effective host: CLI override > workspace.remote >
    // orgs[<org>].remote. Allocate a string only when we fall back to
    // the org-default path so the common case stays cheap.
    let org_host: Option<String> = config.resolved_remote(&name, &workspace);
    let remote = remote_override
        .as_ref()
        .or(workspace.remote.as_ref())
        .or(org_host.as_ref());
    let ports = if !ports_override.is_empty() {
        Some(ports_override.as_slice())
    } else {
        workspace.ports.as_deref()
    };

    let runtime_config = config.merged_runtime_for(&workspace, remote.is_some());
    let mounts = config.merged_mounts(&workspace);
    let idle = config.merged_idle(&workspace);
    let command: Option<Vec<String>> = if !opts.command.is_empty() {
        Some(opts.command.clone())
    } else {
        workspace.command.clone()
    };

    // Effective working directory: CLI override > workspace.remote_dir >
    // org root > workspace.path. Shared by local and remote entry; the
    // remote side keeps the string verbatim (the remote shell expands ~),
    // the local side runs it through tilde expansion so Command::current_dir
    // sees an absolute filesystem path.
    let effective_dir = opts
        .dir
        .clone()
        .or_else(|| config.resolved_remote_dir(&name, &workspace));

    // Snapshot the resolved entry shape into the log. When a new-tab
    // chdir later fails (Windows Terminal / WSL Relay inheriting some
    // path we don't control, etc.), this is the first thing to look at:
    //   - what was the local stash path we registered for this workspace?
    //   - what dir are we handing to the remote?
    //   - which host (or `local`)?
    //   - what's this process's local $PWD when emitting OSC signals?
    tracing::info!(
        workspace = %name,
        workspace_path = %workspace.path,
        effective_dir = ?effective_dir,
        remote = ?remote,
        from_new_tab_hook,
        local_pwd = ?std::env::current_dir().ok(),
        "berth enter resolved"
    );

    if let Some(host) = remote {
        let host = host.clone();
        let _ = berth::lifecycle_state::touch(
            &name,
            Some(&host),
            runtime_name(&runtime_config),
            idle.shutdown_after_seconds,
        );
        if !from_new_tab_hook {
            refresh_remote_session_statuses(&config, &host).await;
        }
        let remote_probe_succeeded = ensure_remote_ready(&mut config, &host, &opts).await?;
        let result = enter_remote(
            name,
            &host,
            path,
            ports,
            &runtime_config,
            &mounts,
            &opts,
            effective_dir.as_deref(),
            command.as_deref(),
            remote_probe_succeeded,
        )
        .await;
        if !from_new_tab_hook {
            refresh_remote_session_statuses(&config, &host).await;
        }
        result
    } else {
        let _ = berth::lifecycle_state::touch(
            &name,
            None,
            runtime_name(&runtime_config),
            idle.shutdown_after_seconds,
        );
        let local_cwd = match effective_dir.as_deref() {
            Some(d) => expand_tilde(d),
            None => path.to_path_buf(),
        };
        if !local_cwd.exists() {
            fs::create_dir_all(&local_cwd)?;
        }
        let result = enter_local(
            &name,
            &local_cwd,
            &runtime_config,
            &mounts,
            command.as_deref(),
        );
        let _ = berth::lifecycle_state::remove(&name, None);
        result
    }
}

async fn refresh_remote_session_statuses(config: &Config, host: &str) {
    let workspaces: Vec<String> = config
        .workspaces
        .iter()
        .filter(|(name, ws)| config.resolved_remote(name, ws).as_deref() == Some(host))
        .map(|(name, _)| name.clone())
        .collect();
    if workspaces.is_empty() {
        return;
    }

    let mut script = String::from(
        r#"b="$HOME/.local/bin/berth"
if [ ! -x "$b" ]; then exit 0; fi
"#,
    );
    for ws in &workspaces {
        let quoted = shell_quote(ws);
        script.push_str("printf '%s\\t' ");
        script.push_str(&quoted);
        script.push('\n');
        script.push_str("BERTH_ATTACH_LOCAL=1 \"$b\" attach --session-counts ");
        script.push_str(&quoted);
        script.push_str(" 2>/dev/null || printf '0\\t0\\t0\\n'\n");
    }

    let Ok(out) = ssh::run_remote_command(host, &script).await else {
        return;
    };
    for line in out.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() != 4 {
            continue;
        }
        let Ok(live) = parts[1].parse::<usize>() else {
            continue;
        };
        let Ok(attached) = parts[2].parse::<usize>() else {
            continue;
        };
        let Ok(exited) = parts[3].parse::<usize>() else {
            continue;
        };
        let _ = berth::lifecycle_state::update_session_status(
            parts[0],
            Some(host),
            live,
            attached,
            exited,
        );
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

/// On the first few hook-driven entries, print a single dim line so the
/// user understands what just happened. The shell hook is silent by
/// design — the new tab "just is" the workspace — but the first time
/// that happens it can read like a teleport. Three reminders is enough
/// to teach the muscle memory.
fn maybe_show_new_tab_hint(workspace: &str) {
    const HINT_LIMIT: u32 = 3;
    let path = match new_tab_hint_path() {
        Some(p) => p,
        None => return,
    };
    let shown: u32 = fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    if shown >= HINT_LIMIT {
        return;
    }
    eprintln!(
        "{} new-tab hook auto-entered '{workspace}'  ({}/{HINT_LIMIT}; \
         set `defaults.new_tab_auto_entry: false` in config or \
         `export BERTH_SKIP_AUTO=1` to opt out)",
        "↪".dimmed(),
        shown + 1
    );
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&path, (shown + 1).to_string());
}

fn new_tab_hint_path() -> Option<std::path::PathBuf> {
    let base = env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(dirs::state_dir)
        .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("state")))?;
    Some(base.join("berth").join("new-tab-hint-count"))
}

/// Expand a leading `~` or `~/…` to `$HOME/…` so `Command::current_dir`
/// receives an actual filesystem path. Bare `~user` is left alone — only
/// the common case is handled; anything else is treated as literal.
fn expand_tilde(dir: &str) -> std::path::PathBuf {
    if let Some(rest) = dir.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    } else if dir == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    std::path::PathBuf::from(dir)
}

fn enter_local(
    name: &str,
    path: &Path,
    runtime_config: &Runtime,
    mounts: &[berth::config::Mount],
    command: Option<&[String]>,
) -> Result<()> {
    let shell = default_shell();

    berth::terminal::emit_enter_signals(&berth::terminal::EnterSignal {
        workspace: name,
        dir: None,
        command,
        session_id: None,
    });

    match runtime_config {
        Runtime::Bare => {
            let mut child = match command {
                Some(argv) if !argv.is_empty() => {
                    let mut cmd = std::process::Command::new(&argv[0]);
                    cmd.args(&argv[1..]);
                    cmd.current_dir(path)
                        .env("BERTH_WORKSPACE", name)
                        .env("BERTH_PATH", path.to_string_lossy().as_ref())
                        .spawn()?
                }
                _ => std::process::Command::new(&shell)
                    .current_dir(path)
                    .env("BERTH_WORKSPACE", name)
                    .env("BERTH_PATH", path.to_string_lossy().as_ref())
                    .spawn()?,
            };

            let status = child.wait()?;
            if !status.success() {
                anyhow::bail!("local workspace command exited with error");
            }
        }
        Runtime::Podman(podman) => {
            runtime::validate_configured_mounts(mounts)?;
            let spec = podman_enter_spec(name, path, &shell, podman, mounts, command)?;
            let status = runtime::run_command(&spec)?;
            if !status.success() {
                anyhow::bail!("Podman environment exited with error");
            }
        }
        Runtime::KubernetesPod(kubernetes) => {
            let spec = kubernetes_enter_spec(name, &shell, kubernetes, command)?;
            let status = runtime::run_command(&spec)?;
            if !status.success() {
                anyhow::bail!("Kubernetes pod environment exited with error");
            }
        }
        Runtime::Auto => anyhow::bail!("Auto runtime was not resolved before local entry"),
    }
    berth::terminal::emit_exit_signals(name);
    Ok(())
}

fn default_shell() -> String {
    env::var("SHELL")
        .or_else(|_| env::var("COMSPEC"))
        .unwrap_or_else(|_| {
            if cfg!(windows) {
                "cmd.exe".to_string()
            } else {
                "/bin/bash".to_string()
            }
        })
}

#[allow(clippy::too_many_arguments)]
async fn enter_remote(
    name: String,
    host: &str,
    _path: &Path,
    ports: Option<&[u16]>,
    runtime_config: &Runtime,
    mounts: &[berth::config::Mount],
    opts: &EnterOptions,
    remote_dir: Option<&str>,
    command: Option<&[String]>,
    remote_probe_succeeded: bool,
) -> Result<()> {
    if let Some(ports) = ports {
        let _tunnel = ssh::start_tunnel(host, &name, ports).await?;
    }

    let mut session_id = berth::session::new_session_id();

    // Capture the exact remote supervisor identity for hook replay.
    // Command shape still matters for the first connection, but once a
    // remote supervisor exists the durable thing to recover is its
    // session id, regardless of whether PID 1 is a shell, sudo, or an
    // interactive command such as Claude.
    berth::terminal::emit_enter_signals(&berth::terminal::EnterSignal {
        workspace: &name,
        dir: remote_dir,
        command,
        session_id: Some(&session_id),
    });

    tracing::info!(
        plain = opts.plain,
        session_id = %session_id,
        session_id_reused_on_reconnect = !opts.plain,
        has_dir = remote_dir.is_some(),
        has_cmd = command.is_some(),
        no_reconnect = opts.no_reconnect,
        "starting remote ssh session"
    );

    // Auto-reconnect / session-recovery loop.
    //
    // Two failure modes are handled, and they are deliberately NOT the
    // same:
    //
    //   * SSH exit status 255 = transport loss (the network dropped, the
    //     laptop slept). The remote supervisor is presumably still alive,
    //     so we re-run ssh+attach against the SAME session id until the
    //     link returns and the command exits cleanly, or the user Ctrl+Cs.
    //     This can wait indefinitely — that is the point of a resumable
    //     session.
    //
    //   * SESSION_NOT_FOUND_EXIT from an attach-only reconnect = the exact
    //     supervisor we were attaching to is GONE (idle-shutdown, reboot,
    //     OOM while we were away overnight). Retrying that dead id forever
    //     is useless — the previous behaviour spun on "still waiting for
    //     the same remote session" without end. Instead we retry a small,
    //     bounded number of times to absorb a supervisor-mid-restart race,
    //     then give up on the id, mint a FRESH session, and create it so
    //     the user lands back in a working workspace.
    //
    // The first SSH creates the session; once it exists every reconnect is
    // attach-only so a blip never forks a duplicate. A future `berth enter`
    // (a new tab) always starts from a brand-new id, giving a distinct but
    // identical session.
    const MAX_SESSION_GONE_RETRIES: u32 = 3;
    const MAX_FRESH_RESTARTS: u32 = 3;
    let (backoff_start, backoff_cap) = reconnect_backoff_params();
    let mut backoff_ms: u64 = backoff_start;
    let mut attempt: u32 = 0;
    // False until a session has (probably) been created remotely; once set,
    // reconnects attach-only and never implicitly create a replacement.
    let mut attach_only = false;
    let mut session_gone_retries: u32 = 0;
    let mut fresh_restarts: u32 = 0;
    // Assigned at the top of every iteration before any `break`, so the
    // post-loop diagnostic always reads a real value.
    let mut last_reconnect_attach_only;
    let final_code = loop {
        attempt += 1;
        let reconnect_attach_only = attach_only && !opts.plain;
        last_reconnect_attach_only = reconnect_attach_only;
        let result = if opts.plain {
            ssh::ssh_interactive(host, &name, true).await
        } else {
            let overrides = ssh::RemoteEnterOverrides {
                remote_dir,
                command,
                session_id: Some(&session_id),
                session_mode: if reconnect_attach_only {
                    RemoteSessionMode::AttachOnly
                } else {
                    RemoteSessionMode::CreateOrAttach
                },
            };
            ssh::ssh_interactive_runtime_with(host, &name, runtime_config, mounts, overrides).await
        };
        let code = result?;
        tracing::info!(
            code,
            attempt,
            session_id = %session_id,
            reused_session_id = reconnect_attach_only,
            reconnect_attach_only,
            "remote ssh session returned"
        );

        // Clean exit, or the user opted out of reconnect / is in --plain
        // mode (no resumable session to recover): nothing more to do.
        if code == 0 {
            break code;
        }
        if opts.plain {
            tracing::warn!(
                workspace = %name,
                host,
                session_id = %session_id,
                attempt,
                code,
                "plain remote ssh returned non-zero; not entering reconnect loop"
            );
            break code;
        }
        if opts.no_reconnect {
            tracing::warn!(
                workspace = %name,
                host,
                session_id = %session_id,
                attempt,
                code,
                "remote ssh returned non-zero; reconnect disabled"
            );
            break code;
        }

        if code == 255 {
            // Transport loss. On the very first attempt, if the preflight
            // reachability probe also failed, this is an SSH transport/setup
            // problem rather than a mid-session blip — don't spin on it.
            if !attach_only && !remote_probe_succeeded {
                tracing::warn!(
                    workspace = %name,
                    host,
                    session_id = %session_id,
                    attempt,
                    "remote host was not reachable during preflight; not entering reconnect loop"
                );
                break code;
            }
            let was_attach_only = attach_only;
            // A session may now exist remotely; future attempts attach to
            // it instead of creating a duplicate. Transport loss is
            // unrelated to a missing session, so reset that counter.
            attach_only = true;
            session_gone_retries = 0;
            tracing::warn!(
                workspace = %name,
                host,
                session_id = %session_id,
                attempt,
                backoff_ms,
                "remote ssh transport lost; reconnecting to the same session"
            );
            restore_terminal_modes_for_status();
            if !was_attach_only {
                eprintln!(
                    "{} connection lost while starting session {}; reconnecting to the same session…  (Ctrl+C to abort)",
                    "·".dimmed(),
                    session_id.cyan()
                );
            } else if attempt.is_multiple_of(4) {
                eprintln!(
                    "{} still reconnecting session {} (attempt {attempt})…",
                    "·".dimmed(),
                    session_id.cyan()
                );
            } else {
                eprintln!(
                    "{} connection lost; reconnecting session {}…  (Ctrl+C to abort)",
                    "·".dimmed(),
                    session_id.cyan()
                );
            }
            tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
            backoff_ms = (backoff_ms.saturating_mul(2)).min(backoff_cap);
            continue;
        }

        // Non-zero, non-255: the remote command actually ran. The only case
        // we recover from is "the session I was attaching to is gone";
        // anything else (the attached program exited non-zero, a genuine
        // failure) is propagated.
        if reconnect_attach_only && code == berth::session::SESSION_NOT_FOUND_EXIT {
            session_gone_retries += 1;
            if session_gone_retries <= MAX_SESSION_GONE_RETRIES {
                // Brief flakiness window — a supervisor may be restarting.
                tracing::warn!(
                    workspace = %name,
                    host,
                    session_id = %session_id,
                    attempt,
                    session_gone_retries,
                    backoff_ms,
                    "remote session not found; retrying briefly before giving up on this id"
                );
                restore_terminal_modes_for_status();
                eprintln!(
                    "{} session {} not found on remote; retrying ({}/{})…  (Ctrl+C to abort)",
                    "·".dimmed(),
                    session_id.cyan(),
                    session_gone_retries,
                    MAX_SESSION_GONE_RETRIES
                );
                tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms.saturating_mul(2)).min(backoff_cap);
                continue;
            }

            // The session is genuinely gone. Give up on the dead id and
            // start fresh — but bound how many times we'll do this so an
            // unstable remote can't trap us in a create→die→create cycle.
            fresh_restarts += 1;
            if fresh_restarts > MAX_FRESH_RESTARTS {
                tracing::error!(
                    workspace = %name,
                    host,
                    session_id = %session_id,
                    attempt,
                    fresh_restarts,
                    "remote session repeatedly lost after restart; giving up"
                );
                restore_terminal_modes_for_status();
                eprintln!(
                    "{} remote session keeps disappearing after {} fresh starts; giving up",
                    "✗".red().bold(),
                    MAX_FRESH_RESTARTS
                );
                break code;
            }
            let gone = std::mem::replace(&mut session_id, berth::session::new_session_id());
            attach_only = false;
            session_gone_retries = 0;
            backoff_ms = backoff_start;
            tracing::warn!(
                workspace = %name,
                host,
                old_session = %gone,
                new_session = %session_id,
                attempt,
                fresh_restarts,
                "remote session gone; minting a fresh session"
            );
            restore_terminal_modes_for_status();
            eprintln!(
                "{} remote session {} is gone; starting a fresh session {}",
                "·".dimmed(),
                gone.cyan(),
                session_id.cyan()
            );
            // Refresh the new-tab breadcrumb + title so they reflect the
            // session the user is actually in now.
            berth::terminal::emit_enter_signals(&berth::terminal::EnterSignal {
                workspace: &name,
                dir: remote_dir,
                command,
                session_id: Some(&session_id),
            });
            continue;
        }

        tracing::info!(
            workspace = %name,
            host,
            session_id = %session_id,
            final_code = code,
            "remote ssh session finished without reconnect"
        );
        break code;
    };

    berth::terminal::emit_exit_signals(&name);
    tracing::info!(
        workspace = %name,
        host,
        session_id = %session_id,
        final_code,
        "emitted exit signals"
    );

    if final_code != 0 {
        let final_reconnect_attach_only = last_reconnect_attach_only;
        // The full ambiguity breakdown is verbose and only useful when
        // debugging; keep it in the log file (captured at warn) rather than
        // dumping a paragraph on the user's terminal on every failed enter.
        let diagnostic = remote_exit_diagnostic(
            &name,
            host,
            &session_id,
            final_code,
            attempt,
            final_reconnect_attach_only,
            opts.no_reconnect,
            remote_probe_succeeded,
            opts.plain,
        );
        tracing::warn!(
            workspace = %name,
            host,
            session_id = %session_id,
            final_code,
            attempts = attempt,
            reconnect_attach_only = final_reconnect_attach_only,
            remote_probe_succeeded,
            no_reconnect = opts.no_reconnect,
            plain = opts.plain,
            exit_phase = diagnostic.phase,
            ssh_status_255_ambiguous = final_code == 255,
            "remote session exited with error: {}",
            diagnostic.message
        );
        let from_hook = env::var_os("BERTH_FROM_HOOK").is_some();
        anyhow::bail!(
            "{}",
            concise_remote_failure_message(&name, host, &session_id, final_code, from_hook)
        );
    }
    Ok(())
}

/// Short, actionable failure shown to the user when a remote enter can't be
/// established. The exhaustive 255-is-ambiguous explanation goes to
/// `berth logs`; here we point at the two commands worth copy-pasting plus
/// where to read the detail. Hook-driven (new-tab) entries get a single line
/// since the shell hook already appends its own "Skipping" note.
fn concise_remote_failure_message(
    workspace: &str,
    host: &str,
    session_id: &str,
    code: i32,
    from_hook: bool,
) -> String {
    let reason = if code == 255 {
        "ssh exited 255 (transport or remote setup)".to_string()
    } else {
        format!("remote exited {code}")
    };
    let attach = format!("berth attach --session {session_id} {workspace}");
    let logs = "berth logs";
    if from_hook {
        format!(
            "{} couldn't enter '{workspace}' on {host} ({reason}) — try `{}`, details in `{logs}`",
            "✗".red().bold(),
            attach.cyan(),
        )
    } else {
        format!(
            "{} couldn't start the remote session for '{workspace}' on {host} ({reason})\n    \
             reconnect:  {}\n    retry:      {}\n    details:    {}",
            "✗".red().bold(),
            attach.cyan(),
            format!("berth enter {workspace}").cyan(),
            logs.cyan(),
        )
    }
}

struct RemoteExitDiagnostic {
    phase: &'static str,
    message: String,
}

#[allow(clippy::too_many_arguments)]
fn remote_exit_diagnostic(
    workspace: &str,
    host: &str,
    session_id: &str,
    code: i32,
    attempts: u32,
    reconnect_attach_only: bool,
    no_reconnect: bool,
    remote_probe_succeeded: bool,
    plain: bool,
) -> RemoteExitDiagnostic {
    if code != 255 {
        return RemoteExitDiagnostic {
            phase: "remote-command-exit",
            message: format!("remote exited with status {code}"),
        };
    }

    let quoted_host = ssh::shell_escape_arg(host);

    if plain {
        return RemoteExitDiagnostic {
            phase: "plain-ssh-status-255",
            message: format!(
                "plain SSH to {host} exited with status 255. SSH reserves 255 for transport/setup failures, but a remote shell command can also return 255. Run `ssh -tt {quoted_host}` to inspect SSH errors, or rerun without `--plain` for berth's resumable attach diagnostics."
            ),
        };
    }

    if reconnect_attach_only {
        let quoted_session_id = ssh::shell_escape_arg(session_id);
        let quoted_workspace = ssh::shell_escape_arg(workspace);
        let quoted_remote_attach = ssh::shell_escape_arg(&format!(
            "berth attach --session {quoted_session_id} {quoted_workspace}"
        ));
        return RemoteExitDiagnostic {
            phase: "reconnect-attach-status-255",
            message: format!(
                "remote SSH/attach returned status 255 while reconnecting to session {session_id} for workspace '{workspace}' on {host} after {attempts} attempts. SSH uses 255 for transport loss, but a remote attach command can also return 255; retry, or run `ssh {quoted_host} {quoted_remote_attach}` to inspect the remote attach path."
            ),
        };
    }

    if no_reconnect {
        return RemoteExitDiagnostic {
            phase: "initial-status-255-no-reconnect",
            message: format!(
                "remote SSH/attach returned status 255 while starting session {session_id} for workspace '{workspace}' on {host}. SSH uses 255 for transport/setup failures, but a remote attach command can also return 255; `--no-reconnect` is set, so berth did not retry. Retry without `--no-reconnect`, or run `ssh -tt {quoted_host}` to inspect SSH errors."
            ),
        };
    }

    if !remote_probe_succeeded {
        let quoted_workspace = ssh::shell_escape_arg(workspace);
        return RemoteExitDiagnostic {
            phase: "initial-transport-status-255-preflight-failed",
            message: format!(
                "remote SSH returned status 255 while starting session {session_id} for workspace '{workspace}' on {host}. The reachability preflight also failed, so this is most likely an SSH transport/setup failure and berth did not enter the reconnect loop. Check connectivity with `ssh {quoted_host}`, then retry `berth enter --remote {quoted_host} {quoted_workspace}`."
            ),
        };
    }

    RemoteExitDiagnostic {
        phase: "initial-status-255",
        message: format!(
            "remote SSH/attach returned status 255 while starting session {session_id} for workspace '{workspace}' on {host}. SSH uses 255 for transport loss, but a remote attach command can also return 255; retry, or run `ssh -tt {quoted_host}` to inspect SSH errors."
        ),
    }
}

/// Initial and capped backoff (ms) for the reconnect / session-recovery
/// loop. Defaults to 500ms → 10s, overridable via `BERTH_RECONNECT_BACKOFF_MS`
/// and `BERTH_RECONNECT_BACKOFF_CAP_MS` (operators who want a snappier or
/// gentler retry cadence, and the test suite to keep its sleeps tiny). The
/// cap is floored to the start so a misconfiguration can't invert them.
fn reconnect_backoff_params() -> (u64, u64) {
    let start = env::var("BERTH_RECONNECT_BACKOFF_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(500);
    let cap = env::var("BERTH_RECONNECT_BACKOFF_CAP_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(10_000)
        .max(start);
    (start, cap)
}

fn restore_terminal_modes_for_status() {
    if !std::io::stderr().is_terminal() {
        return;
    }
    print!("{}", terminal_status_restore_sequence());
    let _ = std::io::stdout().flush();
}

fn terminal_status_restore_sequence() -> &'static str {
    // Leave alternate screen, show cursor, disable common mouse modes
    // and bracketed paste, reset styling, then start the status on a
    // fresh line. Avoid RIS/full clear so scrollback and prompt context
    // survive reconnect churn.
    "\x1b[?1049l\x1b[?25h\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?2004l\x1b[0m\r\n"
}

fn podman_enter_spec(
    name: &str,
    path: &Path,
    shell: &str,
    podman: &berth::config::PodmanRuntime,
    mounts: &[berth::config::Mount],
    command: Option<&[String]>,
) -> Result<CommandSpec> {
    let runtime_mounts = mounts
        .iter()
        .map(|mount| {
            if mount.readonly {
                berth::runtime::ConfiguredMount::new(&mount.source, &mount.target)
            } else {
                berth::runtime::ConfiguredMount::read_write(&mount.source, &mount.target)
            }
        })
        .collect::<Vec<_>>();

    let entry_command = command
        .filter(|argv| !argv.is_empty())
        .map(|argv| argv.to_vec())
        .unwrap_or_else(|| vec![shell.to_string()]);
    let mut config =
        berth::runtime::podman::PodmanRunConfig::new(&podman.image, path, entry_command)
            .with_mounts(runtime_mounts);
    config.project = config
        .project
        .with_target(std::path::PathBuf::from(&podman.project_mount));
    let mut spec = berth::runtime::podman::build_command(&config)?;
    spec.program = podman.binary.clone();
    let name_arg = format!("berth-{}", name.replace('/', "-"));
    spec.args.splice(1..1, ["--name".to_string(), name_arg]);
    if let Some(userns) =
        berth::discovery::podman_userns_arg(&podman.binary, podman.userns.as_deref())
    {
        spec.args.splice(1..1, [userns]);
    }
    Ok(spec)
}

fn runtime_name(runtime_config: &Runtime) -> &'static str {
    match runtime_config {
        Runtime::Bare => "bare",
        Runtime::Podman(_) => "podman",
        Runtime::KubernetesPod(_) => "kubernetes-pod",
        Runtime::Auto => "auto",
    }
}

fn kubernetes_enter_spec(
    name: &str,
    shell: &str,
    kubernetes: &berth::config::KubernetesPodRuntime,
    command: Option<&[String]>,
) -> Result<CommandSpec> {
    let entry_command = command
        .filter(|argv| !argv.is_empty())
        .map(|argv| argv.to_vec())
        .unwrap_or_else(|| vec![shell.to_string()]);
    Ok(berth::runtime::kubernetes::build_run_command(
        &berth::runtime::kubernetes::KubernetesRunConfig::new(
            name,
            kubernetes.clone(),
            entry_command,
        ),
    )?)
}

/// Implement the resumability cascade for remote enter.
///
///   --plain                  → no-op (caller will run plain ssh)
///   --no-deploy              → no-op; SSH cascade will pick mosh/tmux/screen
///                              or plain shell
///   trusted_hosts contains host → silent redeploy if remote is missing/stale
///   --auto-deploy            → deploy without prompt
///   default                  → probe; if remote needs work, prompt the user
///                              (TTY only); on accept, deploy and trust
async fn ensure_remote_ready(config: &mut Config, host: &str, opts: &EnterOptions) -> Result<bool> {
    if opts.plain {
        eprintln!("berth: --plain set; opening a plain SSH shell with no resumable session");
        return Ok(remote_reachability_probe(host).await);
    }
    if opts.no_deploy {
        return Ok(remote_reachability_probe(host).await);
    }
    let quoted_host = ssh::shell_escape_arg(host);

    // Best-effort nag if the local binary is behind the latest GitHub
    // release; never blocks real work.
    deploy::freshness::warn_if_stale().await;

    let local_version = berth::build_info::version().to_string();
    let local_build = berth::build_info::build_id();
    let env = match deploy::probe(host).await {
        Ok(env) => env,
        Err(err) => {
            eprintln!(
                "berth: probe of {host} failed ({err:#}); falling through to the SSH cascade"
            );
            return Ok(false);
        }
    };

    let decision = deploy::decide(&env, &local_version, local_build);
    let already_trusted = config.trusted_hosts.contains_key(host);

    // Only surface the version when there's something noteworthy: a
    // drift between local and remote, or a remote we're about to touch.
    // Quiet runs (matching versions, no deploy decision) say nothing.
    let remote_ver_str = env
        .berth_version
        .as_deref()
        .map(|v| match env.berth_build.as_deref() {
            Some(build) => format!("berth {v} ({build})"),
            None => format!("berth {v}"),
        })
        .unwrap_or_else(|| "no remote berth".to_string());
    // Only surface the version banner on a genuine VERSION difference. A
    // build-id-only drift (two `-dirty` builds of the same release, common
    // when developing berth itself) is not worth a line on every enter.
    let version_drift = env.berth_version.as_deref() != Some(local_version.as_str());
    if version_drift {
        eprintln!(
            "{} local v{} ({})  |  {host}: {}",
            "·".dimmed(),
            local_version.cyan(),
            local_build.cyan(),
            remote_ver_str.cyan()
        );
    }

    let consent = match (opts.auto_deploy, already_trusted) {
        (true, _) => ConsentMode::AutoApproved,
        (_, true) if config.auto_update_remote => ConsentMode::AutoApproved,
        (_, true) => {
            // Trusted but auto-update disabled. Print a clear hint and
            // treat this run as no-deploy so the legacy mux cascade
            // takes over with whatever's on the remote.
            // Only nag about a deferred update for a real pending deploy —
            // not for a same-version build refresh we couldn't do anyway.
            if matches!(decision, DeployDecision::Deploy { .. }) {
                eprintln!(
                    "berth: auto_update_remote is false; remote stays at {remote_ver_str}. \
                     Run `berth deploy --force {quoted_host}` to refresh."
                );
            }
            return Ok(true);
        }
        _ => ConsentMode::Ask,
    };

    match decision {
        DeployDecision::UpToDate => Ok(true),
        DeployDecision::UnsupportedArch { os, arch } => {
            anyhow::bail!(
                "berth has no pre-built binary for {os}/{arch} on {host}. \
                 Install tmux/screen on the remote, or rerun with \
                 `berth enter --plain --remote {quoted_host} <ws>` to skip session-mux."
            );
        }
        DeployDecision::LocalBuildUnsupported {
            target,
            local_target,
            reason,
        } => {
            // Same-version, build-id-only difference we can't push across a
            // target mismatch (e.g. an aarch64 dev box driving an x86_64
            // remote). The remote already runs the same release version, so
            // this is benign — keep it out of the user's way and leave the
            // detail for `berth logs`.
            tracing::debug!(
                %reason,
                ?local_target,
                remote_target = %target,
                "skipping cross-target same-version build refresh"
            );
            Ok(true)
        }
        DeployDecision::Deploy {
            target,
            reason,
            source,
        } => {
            if consent == ConsentMode::Ask
                && !confirm_deploy(host, target, &env, &local_version, &reason)?
            {
                eprintln!(
                    "berth: deploy declined; falling through to the SSH cascade. \
                     Use `--plain` to skip session-mux entirely, or \
                     `berth deploy {quoted_host}` later to opt in."
                );
                return Ok(true);
            }
            let info = match source {
                deploy::DeploySource::Release => {
                    let tag = format!("v{local_version}");
                    deploy::ensure_deployed(host, &tag, target)
                        .await
                        .with_context_hard_fail(host)?
                }
                deploy::DeploySource::LocalBinary => deploy::ensure_deployed_local(host, target)
                    .await
                    .with_context_hard_fail(host)?,
            };
            deploy::record_trust(config, host, &info)?;
            eprintln!(
                "{} deployed v{} → {}:{}",
                "✓".green().bold(),
                info.version,
                host,
                info.remote_path.display()
            );
            Ok(true)
        }
    }
}

async fn remote_reachability_probe(host: &str) -> bool {
    ssh::run_remote_command_with_timeout(host, "true", Duration::from_secs(5))
        .await
        .is_ok()
}

fn confirm_deploy(
    host: &str,
    target: &'static str,
    env: &berth::deploy::RemoteEnv,
    local_version: &str,
    reason: &str,
) -> Result<bool> {
    if !io::stdin().is_terminal() {
        // Non-interactive: don't prompt; behave like --no-deploy.
        eprintln!("berth: {host} {reason}; running non-interactively, skipping deploy");
        return Ok(false);
    }
    // Make the arch decision auditable BEFORE the prompt so the user can
    // sanity-check that we're not about to push an x86 binary at an ARM
    // box (or vice versa).
    eprintln!("berth: deploy plan for {host}");
    eprintln!(
        "  local:  {} / {}  (v{local_version})",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    eprintln!(
        "  remote: {} / {}  ({})",
        env.os,
        env.arch,
        env.berth_version
            .as_deref()
            .map(|v| format!("berth v{v}"))
            .unwrap_or_else(|| "no existing berth".to_string())
    );
    eprintln!("  target: {target}");
    eprint!("berth: deploy? [Y/n]: ");
    io::stderr().flush().ok();
    let answer = read_yes_no_default_yes()?;
    eprintln!("{}", if answer { "y" } else { "n" });
    Ok(answer)
}

/// Single-keystroke Y/n prompt with Y as the default. Returns true on
/// Y/y/Enter, false otherwise. Restores the original termios state on
/// every exit path including panics, via a Drop guard.
fn read_yes_no_default_yes() -> Result<bool> {
    #[cfg(windows)]
    {
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        let answer = line.trim();
        return Ok(answer.is_empty() || matches!(answer, "y" | "Y" | "yes" | "YES" | "Yes"));
    }
    #[cfg(unix)]
    {
        use nix::sys::termios::{tcgetattr, tcsetattr, LocalFlags, SetArg, Termios};

        struct RawModeGuard {
            original: Termios,
        }
        impl Drop for RawModeGuard {
            fn drop(&mut self) {
                let stdin = io::stdin();
                let _ = tcsetattr(stdin.as_fd(), SetArg::TCSANOW, &self.original);
            }
        }

        let stdin = io::stdin();
        let original = tcgetattr(stdin.as_fd())?;
        let mut raw = original.clone();
        raw.local_flags
            .remove(LocalFlags::ICANON | LocalFlags::ECHO);
        tcsetattr(stdin.as_fd(), SetArg::TCSANOW, &raw)?;
        let _guard = RawModeGuard { original };

        let mut byte = [0u8; 1];
        let n = stdin.lock().read(&mut byte)?;
        if n == 0 {
            return Ok(true); // EOF — fall to the default
        }
        Ok(matches!(byte[0], b'y' | b'Y' | b'\r' | b'\n'))
    }
}

/// Extension trait that converts a deploy failure into a clear hard-fail
/// pointing the user at the `--plain` escape hatch.
trait ContextHardFail<T> {
    fn with_context_hard_fail(self, host: &str) -> Result<T>;
}

impl<T> ContextHardFail<T> for Result<T> {
    fn with_context_hard_fail(self, host: &str) -> Result<T> {
        let quoted_host = ssh::shell_escape_arg(host);
        self.map_err(|e| {
            anyhow::anyhow!(
                "deploy to {host} failed: {e:#}\n\
                 Workarounds:\n  \
                 • `berth enter --plain --remote {quoted_host} <ws>` opens a plain SSH session (no resume)\n  \
                 • install tmux or mosh on {host} and rerun without --no-deploy\n  \
                 • run `berth deploy {quoted_host}` interactively to inspect the failure"
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{remote_exit_diagnostic, terminal_status_restore_sequence};
    use berth::ssh;

    #[test]
    fn reconnect_status_restore_does_not_clear_scrollback() {
        let seq = terminal_status_restore_sequence();

        assert!(seq.contains("\x1b[?1049l"));
        assert!(seq.contains("\x1b[?25h"));
        assert!(seq.contains("\x1b[?2004l"));
        assert!(!seq.contains("\x1b[2J"));
        assert!(!seq.contains("\x1b[!p"));
    }

    #[test]
    fn status_255_diagnostic_names_initial_no_reconnect_phase() {
        let diagnostic = remote_exit_diagnostic(
            "atlas/atlas-docs",
            "agents-k",
            "84af72336e7f",
            255,
            1,
            false,
            true,
            true,
            false,
        );

        assert_eq!(diagnostic.phase, "initial-status-255-no-reconnect");
        assert!(diagnostic
            .message
            .contains("while starting session 84af72336e7f"));
        assert!(diagnostic.message.contains("workspace 'atlas/atlas-docs'"));
        assert!(diagnostic.message.contains("SSH uses 255"));
        assert!(diagnostic.message.contains("--no-reconnect"));
    }

    #[test]
    fn status_255_diagnostic_names_plain_phase_and_quotes_host_command() {
        let diagnostic = remote_exit_diagnostic(
            "atlas/atlas-docs",
            "deploy'host",
            "84af72336e7f",
            255,
            1,
            false,
            false,
            true,
            true,
        );

        assert_eq!(diagnostic.phase, "plain-ssh-status-255");
        assert!(diagnostic.message.contains("plain SSH to deploy'host"));
        let expected_command = format!("Run `ssh -tt {}`", ssh::shell_escape_arg("deploy'host"));
        assert!(diagnostic.message.contains(&expected_command));
    }

    #[test]
    fn status_255_reconnect_diagnostic_quotes_pasteable_ssh_command() {
        let quoted_host = ssh::shell_escape_arg("deploy'host");
        let quoted_remote_attach = ssh::shell_escape_arg(&format!(
            "berth attach --session {} {}",
            ssh::shell_escape_arg("session'42"),
            ssh::shell_escape_arg("atlas/atlas-docs")
        ));
        let expected_command = format!("run `ssh {quoted_host} {quoted_remote_attach}`");

        let diagnostic = remote_exit_diagnostic(
            "atlas/atlas-docs",
            "deploy'host",
            "session'42",
            255,
            2,
            true,
            false,
            true,
            false,
        );

        assert_eq!(diagnostic.phase, "reconnect-attach-status-255");
        assert!(diagnostic.message.contains(&expected_command));
        assert!(diagnostic.message.contains("atlas/atlas-docs"));
    }
}
