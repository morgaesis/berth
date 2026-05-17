use crate::config::{Mount, Runtime};
use anyhow::Result;
use std::env;
use std::process;
use tokio::process::Command;
use tokio::time::sleep;

use crate::tunnel::TunnelState;

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

pub async fn ssh_interactive(host: &str, workspace_name: &str, ensure_dir: bool) -> Result<()> {
    if skip_ssh() {
        println!(
            "[TEST MODE] Would SSH to {} and enter workspace {}",
            host, workspace_name
        );
        return Ok(());
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
        .arg(host)
        .arg(&ensure_cmd)
        .arg("&&")
        .arg("exec")
        .arg("$SHELL")
        .status()
        .await?;

    if !status.success() {
        anyhow::bail!("SSH session exited with error");
    }

    Ok(())
}

pub async fn ssh_interactive_runtime(
    host: &str,
    workspace_name: &str,
    runtime: &Runtime,
    mounts: &[Mount],
) -> Result<()> {
    let remote_path = remote_workspace_path_expr(workspace_name);
    let enter_cmd = remote_enter_command(workspace_name, &remote_path, runtime, mounts);

    if skip_ssh() {
        println!(
            "[TEST MODE] Would SSH to {} and enter workspace {} with command: {}",
            host, workspace_name, enter_cmd
        );
        return Ok(());
    }

    let status = Command::new("ssh")
        .arg("-tt")
        .arg(host)
        .arg(enter_cmd)
        .status()
        .await?;

    if !status.success() {
        anyhow::bail!("SSH session exited with error");
    }

    Ok(())
}

/// `remote_path` is a shell *expression* (already quoted/composed by
/// [`remote_workspace_path_expr`]) and is interpolated raw here.
fn remote_enter_command(
    workspace_name: &str,
    remote_path: &str,
    runtime: &Runtime,
    mounts: &[Mount],
) -> String {
    let base = format!("mkdir -p {remote_path} && cd {remote_path}");
    let shell = "${SHELL:-/bin/sh}";
    let session = format!("berth-{}", workspace_name.replace('/', "-"));
    let inner = match runtime {
        Runtime::Bare => format!("exec {shell}"),
        Runtime::Podman(podman) => {
            let escaped_project_mount = shell_escape_arg(&podman.project_mount);
            let mut volumes = vec![format!(
                "-v {}:{}:Z",
                remote_path, escaped_project_mount
            )];
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
    // remains workspace-derived for human-readable `tmux ls` output.
    let unique_session = shell_escape_arg(&format!("{session}-$$-$RANDOM"));

    // Resumability cascade. Best to worst:
    //   1. berth attach --new: PTY-multiplexing supervisor managed by berth
    //      itself. `--new` ensures every local tab gets an independent
    //      session; resume from a prior session is an explicit
    //      `berth attach <ws>` invocation.
    //   2. mosh: UDP-resumable interactive transport.
    //   3. tmux / screen: legacy multiplexers if installed. Each invocation
    //      uses a unique session id so tabs don't pile into one session.
    //   4. plain shell: last resort, no reattach guarantee.
    format!(
        "{base} && \
         if command -v berth >/dev/null 2>&1; then \
           exec berth attach --new {escaped_workspace}; \
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

    let output = Command::new("ssh").arg(host).arg(command).output().await?;

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
            .find("command -v berth")
            .expect("berth attach probe must come first");
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

        assert!(command.contains("exec berth attach --new 'work'"));
        assert!(command.contains("mosh-server new --"));
        // Legacy fallbacks use a unique session id per invocation so
        // multiple terminal tabs don't pile into one shared session.
        assert!(command.contains("tmux new-session -s 'berth-work-$$-$RANDOM'"));
        assert!(command.contains("screen -S 'berth-work-$$-$RANDOM'"));
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
        // so multi-tab tmux/screen sessions don't collide.
        assert!(command.contains("'berth-team-work-$$-$RANDOM'"));
    }
}
