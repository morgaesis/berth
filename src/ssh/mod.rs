use crate::config::{Mount, Runtime};
use anyhow::Result;
use std::env;
use std::process;
use tokio::process::Command;
use tokio::time::sleep;

use crate::tunnel::TunnelState;

mod osc7_filter;
mod pty_proxy;

fn skip_ssh() -> bool {
    env::var("BERTH_SKIP_SSH").is_ok()
}

fn remote_projects_path() -> &'static str {
    "$HOME/.local/share/berth/projects"
}

/// Build the remote-side workspace path as a shell expression: the controlled
/// prefix (which intentionally contains `$HOME` for the remote shell to
/// expand) is left unquoted, while the user-supplied workspace name segment
/// is shell-quoted so it cannot break out of the argument.
///
/// Result form: `"$HOME"/.local/share/berth/projects/'workspace_name'`
/// (the prefix uses `"$HOME"` rather than bare `$HOME` so values containing
/// whitespace stay one token after expansion).
fn remote_workspace_path_expr(workspace_name: &str) -> String {
    // remote_projects_path() is a static literal we control; the only
    // metacharacter is `$HOME` which is intentional. Wrap `$HOME` in
    // double quotes so word-splitting on its expansion is safe; the rest
    // of the prefix is ASCII path chars.
    let prefix = remote_projects_path().replacen("$HOME", "\"$HOME\"", 1);
    format!("{}/{}", prefix, shell_escape_arg(workspace_name))
}

/// `Ok(code)` carries the remote command's exit code (or the SSH
/// transport error code: 255 = connection lost). Caller decides
/// whether to bail or retry; ssh_interactive used to bail itself but
/// that made the auto-reconnect loop impossible without parsing
/// error strings.
pub async fn ssh_interactive(host: &str, workspace_name: &str, ensure_dir: bool) -> Result<i32> {
    if skip_ssh() {
        println!(
            "[TEST MODE] Would SSH to {} and enter workspace {}",
            host, workspace_name
        );
        return Ok(0);
    }

    let remote_path = remote_workspace_path_expr(workspace_name);
    let ensure_cmd = if ensure_dir {
        format!(
            "mkdir -p {0} && cd {0} && export PS1='[berth] $ ' && export PROMPT_COMMAND='PS1=\"[berth] \\u@\\h:\\w\\$ \"'",
            remote_path
        )
    } else {
        format!(
            "cd {0} && export PS1='[berth] $ ' && export PROMPT_COMMAND='PS1=\"[berth] \\u@\\h:\\w\\$ \"'",
            remote_path
        )
    };

    let status = Command::new("ssh")
        .arg("-tt")
        .arg("-o")
        .arg("LogLevel=ERROR")
        .arg(host)
        .arg(&ensure_cmd)
        .arg("&&")
        .arg("exec")
        .arg("$SHELL")
        .status()
        .await?;
    Ok(status.code().unwrap_or(255))
}

/// Optional per-workspace overrides plumbed in from the enter command.
#[derive(Debug, Default, Clone)]
pub struct RemoteEnterOverrides<'a> {
    /// Workspace-supplied remote directory expression (may use `$HOME`,
    /// `~`, etc.). When None, the auto-managed path is used.
    pub remote_dir: Option<&'a str>,
    /// Workspace-supplied default command argv. Forwarded to the remote
    /// `berth attach` cascade arm as trailing `-- <argv...>`.
    pub command: Option<&'a [String]>,
    /// When true, force a fresh independent session (`berth attach --new`).
    /// When false (default), prefer to attach to an existing session so
    /// SSH-drop / hibernation reconnects cleanly
    /// (`berth attach --resume-or-new`).
    pub force_new: bool,
}

pub async fn ssh_interactive_runtime(
    host: &str,
    workspace_name: &str,
    runtime: &Runtime,
    mounts: &[Mount],
) -> Result<i32> {
    ssh_interactive_runtime_with(
        host,
        workspace_name,
        runtime,
        mounts,
        RemoteEnterOverrides::default(),
    )
    .await
}

/// Returns the remote-command exit code (255 for connection lost).
/// Caller decides whether to bail or retry — used by the auto-reconnect
/// loop in `enter` to silently re-run on 255.
pub async fn ssh_interactive_runtime_with(
    host: &str,
    workspace_name: &str,
    runtime: &Runtime,
    mounts: &[Mount],
    overrides: RemoteEnterOverrides<'_>,
) -> Result<i32> {
    let remote_path = match overrides.remote_dir {
        Some(dir) => normalize_remote_path(dir),
        None => remote_workspace_path_expr(workspace_name),
    };
    let enter_cmd = remote_enter_command_with(
        workspace_name,
        &remote_path,
        runtime,
        mounts,
        overrides.command,
        overrides.force_new,
    );

    if skip_ssh() {
        println!(
            "[TEST MODE] Would SSH to {} and enter workspace {} with command: {}",
            host, workspace_name, enter_cmd
        );
        return Ok(0);
    }

    tracing::info!(host = %host, "spawning ssh -tt via osc7-filter wrapper");
    // Run ssh under a local PTY pair and pipe its stdout through an
    // OSC 7 stripper before forwarding to the user's terminal. The
    // remote shell's `precmd`/`PROMPT_COMMAND` typically emits
    // OSC 7 reporting its (remote) cwd; the local terminal would
    // otherwise record that path and try to chdir into it for any
    // new tab the user opens. Stripping the sequence at the ssh
    // boundary keeps berth's own marker dir (emitted before the ssh
    // child starts) authoritative for new-tab inheritance.
    let args: Vec<String> = vec![
        "-tt".into(),
        // Suppress ssh's own status lines ("Shared connection …
        // closed", motd banners). Errors still print.
        "-o".into(),
        "LogLevel=ERROR".into(),
        host.to_string(),
        enter_cmd,
    ];
    // The proxy is sync (portable-pty's APIs are sync); shove it onto
    // a blocking pool so the tokio scheduler doesn't park.
    let code = tokio::task::spawn_blocking(move || pty_proxy::ssh_through_filter(&args))
        .await
        .map_err(|e| anyhow::anyhow!("ssh proxy task: {e:#}"))??;
    tracing::info!(code, "ssh exited");
    Ok(code)
}

/// Convenience wrapper retained for tests; the override-aware variant
/// is the one production code uses.
#[cfg(test)]
fn remote_enter_command(
    workspace_name: &str,
    remote_path: &str,
    runtime: &Runtime,
    mounts: &[Mount],
) -> String {
    remote_enter_command_with(workspace_name, remote_path, runtime, mounts, None, false)
}

/// `remote_path` is a shell *expression* (already quoted/composed by
/// [`remote_workspace_path_expr`]) and is interpolated raw here.
///
/// `force_new` selects the attach verb passed to the remote berth:
/// false (default) → `attach --resume-or-new` (smart resume), true →
/// `attach --new` (always fresh). The new-tab auto-entry hook passes
/// true; `berth enter` passes false.
fn remote_enter_command_with(
    workspace_name: &str,
    remote_path: &str,
    runtime: &Runtime,
    mounts: &[Mount],
    workspace_command: Option<&[String]>,
    force_new: bool,
) -> String {
    let base = format!("mkdir -p {remote_path} && cd {remote_path}");
    let shell = "${SHELL:-/bin/sh}";
    let session = format!("berth-{}", workspace_name.replace('/', "-"));
    let inner = match runtime {
        Runtime::Bare => format!("exec {shell}"),
        Runtime::Podman(podman) => {
            let escaped_project_mount = shell_escape_arg(&podman.project_mount);
            let mut volumes = vec![format!("-v {}:{}:Z", remote_path, escaped_project_mount)];
            for mount in mounts {
                let mode = if mount.readonly { "ro" } else { "rw" };
                volumes.push(format!(
                    "-v {}:{}:{mode}",
                    shell_escape_arg(&mount.source),
                    shell_escape_arg(&mount.target)
                ));
            }
            let userns = podman
                .userns
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(|value| format!("--userns={} ", shell_escape_arg(value)))
                .unwrap_or_default();
            format!(
                "exec {} run --rm -it {}--name {} --workdir {} {} {} {shell}",
                shell_escape_arg(&podman.binary),
                userns,
                shell_escape_arg(&session),
                escaped_project_mount,
                volumes.join(" "),
                shell_escape_arg(&podman.image)
            )
        }
        Runtime::KubernetesPod(_) => {
            "printf 'kubernetes pod runtime is not supported over SSH yet' >&2; exit 2".to_string()
        }
        Runtime::Auto => "exec ${SHELL:-/bin/sh}".to_string(),
    };

    let escaped_workspace = shell_escape_arg(workspace_name);
    let escaped_inner = shell_escape_arg(&inner);
    // Per-invocation tmux/screen session id so each `berth enter` from a
    // new local tab gets an independent multiplexer session even on hosts
    // where the preferred `berth` binary is missing. The session prefix
    // is workspace-derived (and shell-quoted), the suffix is the remote
    // shell's `$$` and `$RANDOM` left UNquoted so they expand on the far
    // side. Concatenation of a quoted and unquoted segment yields a
    // single shell word.
    let unique_session = format!("{}-$$-$RANDOM", shell_escape_arg(&session));

    // Trailing `-- <argv...>` for the `berth attach --new` cascade arm,
    // so a per-workspace command (set in config) lands as the session's
    // PID 1 instead of `$SHELL -l`. Each argv element is shell-quoted
    // independently. Empty / None means "no override; supervisor runs
    // $SHELL -l".
    let attach_verb = if force_new {
        "--new"
    } else {
        "--resume-or-new"
    };
    let attach_cmd_suffix = match workspace_command {
        Some(argv) if !argv.is_empty() => {
            let mut s = String::from(" --");
            for a in argv {
                s.push(' ');
                s.push_str(&shell_escape_arg(a));
            }
            s
        }
        _ => String::new(),
    };

    // Resumability cascade. Best to worst:
    //   1. berth attach --new: PTY-multiplexing supervisor managed by berth
    //      itself. `--new` ensures every local tab gets an independent
    //      session; resume from a prior session is an explicit
    //      `berth attach <ws>` invocation.
    //   2. mosh: UDP-resumable interactive transport.
    //   3. tmux / screen: legacy multiplexers if installed. Each invocation
    //      uses a unique session id so tabs don't pile into one session.
    //   4. plain shell: last resort, no reattach guarantee.
    // `command -v berth` alone misses our deployed-to-`~/.local/bin/berth`
    // path on hosts where that dir is only on the *interactive* shell's
    // PATH (e.g. Ubuntu sources it from `.profile`, which `ssh host cmd`
    // does not see). Probe both, with the explicit path as the fallback.
    format!(
        "{base} && \
         berth_bin=; \
         if command -v berth >/dev/null 2>&1; then \
           berth_bin=$(command -v berth); \
         elif [ -x \"$HOME/.local/bin/berth\" ]; then \
           berth_bin=\"$HOME/.local/bin/berth\"; \
         fi; \
         if [ -n \"$berth_bin\" ]; then \
           exec \"$berth_bin\" attach {attach_verb} {escaped_workspace}{attach_cmd_suffix}; \
         elif command -v mosh-server >/dev/null 2>&1; then \
           exec mosh-server new -- sh -lc {escaped_inner}; \
         elif command -v tmux >/dev/null 2>&1; then \
           exec tmux new-session -s {unique_session} {escaped_inner}; \
         elif command -v screen >/dev/null 2>&1; then \
           exec screen -S {unique_session} sh -lc {escaped_inner}; \
         else \
           {inner}; \
         fi"
    )
}

fn shell_escape_arg(input: &str) -> String {
    format!("'{}'", input.replace('\'', "'\"'\"'"))
}

/// Turn a user-supplied remote path into a single shell expression that
/// expands `$HOME` / `~` correctly on the remote and quotes everything
/// else. POSIX `~` expansion only happens *outside* quotes and only at
/// the very start of a word, so we treat a leading `~/` (or bare `~`) as
/// a special token and emit `"$HOME"/...rest...`. Everything else is
/// double-quoted so values like `$HOME/foo/bar` still expand `$HOME`
/// while characters like spaces or semicolons are kept literal.
fn normalize_remote_path(path: &str) -> String {
    if path == "~" {
        return "\"$HOME\"".to_string();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        let rest_escaped = rest.replace('"', "\\\"");
        return format!("\"$HOME\"/\"{rest_escaped}\"");
    }
    let escaped = path.replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
#[test]
fn normalize_remote_path_handles_tilde_and_dollar_home() {
    assert_eq!(normalize_remote_path("~"), "\"$HOME\"");
    assert_eq!(
        normalize_remote_path("~/code/org/proj"),
        "\"$HOME\"/\"code/org/proj\""
    );
    assert_eq!(
        normalize_remote_path("$HOME/code/proj"),
        "\"$HOME/code/proj\""
    );
    assert_eq!(
        normalize_remote_path("/var/work/proj"),
        "\"/var/work/proj\""
    );
}

pub async fn start_tunnel(host: &str, workspace: &str, ports: &[u16]) -> Result<bool> {
    if skip_ssh() {
        println!(
            "[TEST MODE] Would start tunnel to {} for ports {:?}",
            host, ports
        );
        return Ok(true);
    }

    let mut state = TunnelState::load();

    // Check if we already have this tunnel tracked
    let already_running = ports.iter().all(|p| state.has_port(workspace, *p));
    if already_running {
        println!(
            "Tunnel already active for workspace '{}' on ports {:?}",
            workspace, ports
        );
        return Ok(true);
    }

    // Check for port conflicts with OTHER workspaces
    for port in ports {
        if is_port_in_use(*port) {
            // Check if it's one of our tunnels
            if state.has_port(workspace, *port) {
                continue; // Our tunnel, OK
            }
            anyhow::bail!(
                "Port {} is already in use by another process. Choose a different port.",
                port
            );
        }
    }

    let mut args = vec![
        "-N".to_string(),
        "-f".to_string(),
        "-o".to_string(),
        "ServerAliveInterval=60".to_string(),
        "-o".to_string(),
        "ServerAliveCountMax=3".to_string(),
    ];

    for port in ports {
        args.push(format!("-L {}:localhost:{}", port, port));
    }
    args.push(host.to_string());

    let result = process::Command::new("ssh").args(&args).spawn();

    match result {
        Ok(_) => {
            sleep(tokio::time::Duration::from_millis(300)).await;
            // Bookkeeping: record the tunnel
            state.add(workspace, ports);
            let _ = state.save();
            println!("Started tunnel for '{}' on ports {:?}", workspace, ports);
            Ok(true)
        }
        Err(e) => {
            anyhow::bail!("Failed to start tunnel: {}", e);
        }
    }
}

pub fn stop_tunnel(workspace: &str, port: u16) -> Result<()> {
    let mut state = TunnelState::load();

    if !state.has_port(workspace, port) {
        println!("No tunnel found for '{}' on port {}", workspace, port);
        return Ok(());
    }

    // Kill the SSH tunnel process
    let output = process::Command::new("pkill")
        .args(["-f", &format!("ssh -N.*{}", port)])
        .output();

    match output {
        Ok(_) => {
            state.remove_port(workspace, port);
            let _ = state.save();
            println!("Stopped tunnel for '{}' on port {}", workspace, port);
        }
        Err(e) => {
            eprintln!("Failed to stop tunnel: {}", e);
        }
    }

    Ok(())
}

fn is_port_in_use(port: u16) -> bool {
    use std::net::TcpListener;
    TcpListener::bind(format!("127.0.0.1:{}", port)).is_err()
}

pub async fn run_remote_command(host: &str, command: &str) -> Result<String> {
    if skip_ssh() {
        return Ok(format!("[TEST MODE] Would run on {}: {}", host, command));
    }

    tracing::debug!(host = %host, cmd_len = command.len(), "ssh exec");
    let output = Command::new("ssh").arg(host).arg(command).output().await?;
    tracing::debug!(
        host = %host,
        status = ?output.status,
        stdout_len = output.stdout.len(),
        stderr_len = output.stderr.len(),
        "ssh exec returned"
    );

    if !output.status.success() {
        anyhow::bail!(
            "Remote command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Runtime;

    #[test]
    fn remote_entry_cascades_from_berth_attach_through_legacy_to_plain_shell() {
        let path_expr = remote_workspace_path_expr("work");
        let command = remote_enter_command("work", &path_expr, &Runtime::Bare, &[]);

        let attach_idx = command
            .find("berth_bin=")
            .expect("berth bin discovery must come first");
        let mosh_idx = command
            .find("command -v mosh-server")
            .expect("mosh probe present");
        let tmux_idx = command.find("command -v tmux").expect("tmux probe present");
        let screen_idx = command
            .find("command -v screen")
            .expect("screen probe present");
        assert!(
            attach_idx < mosh_idx && mosh_idx < tmux_idx && tmux_idx < screen_idx,
            "cascade order is berth > mosh > tmux > screen"
        );

        // The exec is now via the resolved `$berth_bin` (either `command -v`
        // result or the explicit `~/.local/bin/berth` fallback path).
        // Default verb is --resume-or-new so SSH-drop reattaches cleanly;
        // tabs that want isolation pass `berth enter --new`.
        assert!(command.contains("exec \"$berth_bin\" attach --resume-or-new 'work'"));
        assert!(command.contains("mosh-server new --"));
        // Legacy fallbacks use a unique session id per invocation so
        // multiple terminal tabs don't pile into one shared session.
        // The prefix is shell-quoted; $$ and $RANDOM are deliberately
        // left unquoted so the remote shell expands them at runtime.
        assert!(command.contains("tmux new-session -s 'berth-work'-$$-$RANDOM"));
        assert!(command.contains("screen -S 'berth-work'-$$-$RANDOM"));
        // No attach-or-create flags: each invocation must be a fresh session.
        assert!(!command.contains("new-session -A"));
        assert!(!command.contains("screen -D -RR"));
        assert!(command.contains("else exec ${SHELL:-/bin/sh}; fi"));
    }

    #[test]
    fn remote_workspace_path_keeps_home_expandable_and_quotes_workspace() {
        // $HOME must remain in the unquoted ("$HOME") form so the remote
        // shell expands it; the workspace name is what we shell-quote.
        let p = remote_workspace_path_expr("work");
        assert!(
            p.starts_with("\"$HOME\"/.local/share/berth/projects/"),
            "expected $HOME-prefixed expression, got {p}"
        );
        assert!(p.ends_with("'work'"));
    }

    #[test]
    fn remote_workspace_path_quotes_hostile_workspace_name() {
        // Hostile name passes validation only if a caller forgets to call
        // validate_workspace_name first; this is the second line of defense.
        let p = remote_workspace_path_expr("'; rm -rf /; #");
        let expected_quoted = shell_escape_arg("'; rm -rf /; #");
        assert!(p.ends_with(&expected_quoted), "got {p}");
    }

    #[test]
    fn remote_entry_escapes_hostile_caller_supplied_path() {
        // Older API expected remote_enter_command to escape; new contract
        // is "caller passes a ready shell expression". Verify the function
        // still interpolates the expression as a complete unit.
        let hostile_expr = shell_escape_arg("/tmp/'; rm -rf /; #");
        let command = remote_enter_command("work", &hostile_expr, &Runtime::Bare, &[]);
        assert!(
            command.contains(&format!("mkdir -p {hostile_expr}")),
            "mkdir uses caller's expression as-is: {command}"
        );
        assert!(
            command.contains(&format!("cd {hostile_expr}")),
            "cd uses quoted path: {command}"
        );
        // shell_escape_arg's own contract: every embedded `'` is broken out
        // via `'"'"'` so the result is always a single shell word.
        assert!(hostile_expr.starts_with('\''));
        assert!(hostile_expr.ends_with('\''));
    }

    #[test]
    fn remote_entry_uses_safe_session_name_for_nested_workspace() {
        let path_expr = remote_workspace_path_expr("team/work");
        let command = remote_enter_command("team/work", &path_expr, &Runtime::Bare, &[]);

        // Session name is workspace-derived with a per-invocation suffix
        // so multi-tab tmux/screen sessions don't collide. The prefix is
        // quoted; $$ and $RANDOM are not, so the remote shell expands them.
        assert!(command.contains("'berth-team-work'-$$-$RANDOM"));
    }
}
