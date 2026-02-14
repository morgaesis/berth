use anyhow::Result;
use std::env;
use std::process;
use tokio::process::Command;

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

pub async fn start_tunnel(host: &str, ports: &[u16]) -> Result<()> {
    if skip_ssh() {
        println!("[TEST MODE] Would start tunnel to {} for ports {:?}", host, ports);
        return Ok(());
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

    println!("Starting SSH tunnel: ssh {}", args.join(" "));
    println!("Forwarding ports: {:?}", ports);
    println!("Access via: http://localhost:<port> or http://<workspace>.berth:<port>");
    println!("Press Ctrl+C to stop.");

    // Use std Command with -f to fork to background
    // This returns immediately after forking
    let status = process::Command::new("ssh")
        .args(&args)
        .spawn()?;

    // Give the tunnel time to establish
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Wait for ctrl+c
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            println!("\nStopping tunnel...");
            // Try to kill the ssh process
            let _ = std::process::Command::new("pkill")
                .arg("-f")
                .arg(format!("ssh -N -L.*{}", host))
                .spawn();
        }
    }

    Ok(())
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