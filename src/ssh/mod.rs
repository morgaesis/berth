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

fn remote_projects_path() -> String {
    "$HOME/.local/share/berth/projects".to_string()
}

pub async fn ssh_interactive(host: &str, workspace_name: &str, ensure_dir: bool) -> Result<()> {
    if skip_ssh() {
        println!(
            "[TEST MODE] Would SSH to {} and enter workspace {}",
            host, workspace_name
        );
        return Ok(());
    }

    let remote_path = format!("{}/{}", remote_projects_path(), workspace_name);
    let ensure_cmd = if ensure_dir {
        format!(
            "mkdir -p {} && cd {} && export PS1='[berth] $ ' && export PROMPT_COMMAND='PS1=\"[berth] \\u@\\h:\\w\\$ \"'",
            remote_path, remote_path
        )
    } else {
        format!(
            "cd {} && export PS1='[berth] $ ' && export PROMPT_COMMAND='PS1=\"[berth] \\u@\\h:\\w\\$ \"'",
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
    let remote_path = format!("{}/{}", remote_projects_path(), workspace_name);
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
            let mut volumes = vec![format!("-v {remote_path}:{}:Z", podman.project_mount)];
            for mount in mounts {
                let mode = if mount.readonly { "ro" } else { "rw" };
                volumes.push(format!("-v {}:{}:{mode}", mount.source, mount.target));
            }
            let userns = podman
                .userns
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(|value| format!("--userns={} ", shell_escape_arg(value)))
                .unwrap_or_default();
            format!(
                "exec {} run --rm -it {}--name {} --workdir {} {} {} {shell}",
                podman.binary,
                userns,
                shell_escape_arg(&session),
                podman.project_mount,
                volumes.join(" "),
                podman.image
            )
        }
        Runtime::KubernetesPod(_) => {
            "printf 'kubernetes pod runtime is not supported over SSH yet' >&2; exit 2".to_string()
        }
        Runtime::Auto => "exec ${SHELL:-/bin/sh}".to_string(),
    };

    let escaped_workspace = shell_escape_arg(workspace_name);
    let escaped_session = shell_escape_arg(&session);
    let escaped_inner = shell_escape_arg(&inner);

    // Resumability cascade. Best to worst:
    //   1. berth attach: PTY-multiplexing supervisor managed by berth itself.
    //   2. mosh: UDP-resumable interactive transport.
    //   3. tmux / screen: legacy multiplexers if installed.
    //   4. plain shell: last resort, no reattach guarantee.
    format!(
        "{base} && \
         if command -v berth >/dev/null 2>&1; then \
           exec berth attach {escaped_workspace}; \
         elif command -v mosh-server >/dev/null 2>&1; then \
           exec mosh-server new -- sh -lc {escaped_inner}; \
         elif command -v tmux >/dev/null 2>&1; then \
           exec tmux new-session -A -s {escaped_session} {escaped_inner}; \
         elif command -v screen >/dev/null 2>&1; then \
           exec screen -D -RR -S {escaped_session} sh -lc {escaped_inner}; \
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
        let command = remote_enter_command(
            "work",
            "$HOME/.local/share/berth/projects/work",
            &Runtime::Bare,
            &[],
        );

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

        assert!(command.contains("exec berth attach 'work'"));
        assert!(command.contains("mosh-server new --"));
        assert!(command.contains("tmux new-session -A -s 'berth-work'"));
        assert!(command.contains("screen -D -RR -S 'berth-work'"));
        assert!(command.contains("else exec ${SHELL:-/bin/sh}; fi"));
    }

    #[test]
    fn remote_entry_uses_safe_session_name_for_nested_workspace() {
        let command = remote_enter_command(
            "team/work",
            "$HOME/.local/share/berth/projects/team-work",
            &Runtime::Bare,
            &[],
        );

        assert!(command.contains("'berth-team-work'"));
    }
}
