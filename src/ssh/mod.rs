use anyhow::Result;
use std::env;
use std::process;
use tokio::process::Command;
use tokio::time::sleep;

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

    println!("Starting SSH tunnel for ports {:?}...", ports);

    // Start tunnel in background, don't wait
    let _status = process::Command::new("ssh")
        .args(&args)
        .spawn()?;

    // Wait a moment for tunnel to establish
    sleep(tokio::time::Duration::from_millis(300)).await;

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