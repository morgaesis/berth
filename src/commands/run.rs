use berth::config::Config;
use berth::ssh;
use anyhow::Result;
use std::process::Command;

pub async fn run(name: String, command: Vec<String>, ports: Vec<u16>, remote_override: Option<String>) -> Result<()> {
    let config = Config::load()?;
    
    let workspace = config.workspaces.get(&name)
        .ok_or_else(|| anyhow::anyhow!("Workspace '{}' not found", name))?;

    // Determine remote - use override or workspace config
    let remote = remote_override.or_else(|| workspace.remote.clone());

    // Ports require a remote
    if !ports.is_empty() && remote.is_none() {
        anyhow::bail!("Ports (-p) require a remote. Use --remote or set remote in workspace config.");
    }

    let cmd_str = command.join(" ");
    if cmd_str.is_empty() {
        anyhow::bail!("No command specified");
    }

    match remote {
        Some(host) => {
            // Remote execution
            let tunnel_active = if !ports.is_empty() {
                ssh::start_tunnel(&host, &name, &ports).await?
            } else {
                false
            };

            let remote_path = format!("~/berth/projects/{}", name);
            let full_cmd = format!(
                "cd {} && nohup {} >/dev/null 2>&1 & disown",
                remote_path, cmd_str
            );

            println!("Running on {}: cd {} && {}", host, remote_path, cmd_str);
            
            let output = ssh::run_remote_command(&host, &full_cmd).await?;
            
            if !output.is_empty() {
                println!("{}", output);
            } else {
                println!("Command started successfully.");
            }
            
            if tunnel_active && !ports.is_empty() {
                println!("Tunnel active: http://localhost:{}", ports[0]);
            }
        }
        None => {
            // Local execution
            let local_path = &workspace.path;
            println!("Running locally: cd {} && {}", local_path, cmd_str);
            
            let output = Command::new("sh")
                .arg("-c")
                .arg(format!("cd {} && {}", local_path, cmd_str))
                .output()?;

            if !output.status.success() {
                anyhow::bail!("Command failed: {}", String::from_utf8_lossy(&output.stderr));
            }

            println!("{}", String::from_utf8_lossy(&output.stdout));
        }
    }
    
    Ok(())
}
