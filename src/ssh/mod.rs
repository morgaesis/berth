use anyhow::Result;
use std::path::Path;
use tokio::process::Command;

pub async fn ssh_interactive(host: &str, path: &Path) -> Result<()> {
    let status = Command::new("ssh")
        .arg("-t")
        .arg(host)
        .arg("cd")
        .arg(path)
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
    let mut args = vec!["-N".to_string()];
    
    for port in ports {
        args.push(format!("-L {}:localhost:{}", port, port));
    }
    args.push(host.to_string());

    println!("Starting SSH tunnel: ssh {}", args.join(" "));
    println!("Forwarding ports: {:?}", ports);
    println!("Access via: http://localhost:<port> or http://<workspace>.berth:<port>");

    let mut child = Command::new("ssh")
        .args(&args)
        .spawn()?;

    tokio::select! {
        result = child.wait() => {
            match result {
                Ok(status) if status.success() => Ok(()),
                Ok(status) => anyhow::bail!("SSH tunnel exited with: {}", status),
                Err(e) => anyhow::bail!("SSH tunnel error: {}", e),
            }
        }
        _ = tokio::signal::ctrl_c() => {
            println!("\nStopping tunnel...");
            let _ = child.kill().await;
            Ok(())
        }
    }
}

pub async fn run_remote_command(host: &str, command: &str) -> Result<String> {
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
