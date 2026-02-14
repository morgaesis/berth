use anyhow::Result;
use std::env;
use std::process;
use tokio::process::Command;
use tokio::time::sleep;

use crate::tunnel::TunnelState;

fn skip_ssh() -> bool {
    env::var("BERTH_SKIP_SSH").is_ok()
}

pub async fn ssh_interactive(host: &str, workspace_name: &str, ensure_dir: bool) -> Result<()> {
    if skip_ssh() {
        println!("[TEST MODE] Would SSH to {} and enter workspace {}", host, workspace_name);
        return Ok(());
    }

    let remote_path = format!("~/berth/projects/{}", workspace_name);
    let ensure_cmd = if ensure_dir {
        format!("mkdir -p {} && cd {}", remote_path, remote_path)
    } else {
        format!("cd {}", remote_path)
    };

    let status = Command::new("ssh")
        .arg("-t")
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

pub async fn start_tunnel(host: &str, workspace: &str, ports: &[u16]) -> Result<bool> {
    if skip_ssh() {
        println!("[TEST MODE] Would start tunnel to {} for ports {:?}", host, ports);
        return Ok(true);
    }

    let mut state = TunnelState::load();

    // Check if we already have this tunnel tracked
    let already_running = ports.iter().all(|p| state.has_port(workspace, *p));
    if already_running {
        println!("Tunnel already active for workspace '{}' on ports {:?}", workspace, ports);
        return Ok(true);
    }

    // Check for port conflicts with OTHER workspaces
    for port in ports {
        if is_port_in_use(*port) {
            // Check if it's one of our tunnels
            if state.has_port(workspace, *port) {
                continue; // Our tunnel, OK
            }
            anyhow::bail!("Port {} is already in use by another process. Choose a different port.", port);
        }
    }

    let mut args = vec![
        "-N".to_string(),
        "-f".to_string(),
        "-o".to_string(), "ServerAliveInterval=60".to_string(),
        "-o".to_string(), "ServerAliveCountMax=3".to_string(),
    ];
    
    for port in ports {
        args.push(format!("-L {}:localhost:{}", port, port));
    }
    args.push(host.to_string());

    let result = process::Command::new("ssh")
        .args(&args)
        .spawn();

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

    let output = Command::new("ssh")
        .arg(host)
        .arg(command)
        .output()
        .await?;

    if !output.status.success() {
        anyhow::bail!("Remote command failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
