use berth::config::Config;
use berth::ssh;
use anyhow::Result;

pub async fn run(name: String, command: Vec<String>, ports: Vec<u16>) -> Result<()> {
    let config = Config::load()?;
    
    let workspace = config.workspaces.get(&name)
        .ok_or_else(|| anyhow::anyhow!("Workspace '{}' not found", name))?;

    let remote = workspace.remote.as_ref()
        .ok_or_else(|| anyhow::anyhow!("Workspace '{}' is not remote", name))?;

    let cmd_str = command.join(" ");
    if cmd_str.is_empty() {
        anyhow::bail!("No command specified");
    }

    // Start tunnel first if ports specified
    if !ports.is_empty() {
        println!("Starting tunnel for ports: {:?}", ports);
        ssh::start_tunnel(remote, &ports).await?;
        // Give tunnel time to establish
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }

    let remote_path = format!("~/berth/projects/{}", name);
    let full_cmd = format!("cd {} && {}", remote_path, cmd_str);

    println!("Running on {}: {}", remote, full_cmd);
    
    let output = ssh::run_remote_command(remote, &full_cmd).await?;
    
    println!("{}", output);
    
    // If ports specified, keep tunnel running
    if !ports.is_empty() {
        println!("\nTunnel running. Press Ctrl+C to stop...");
        let _ = tokio::signal::ctrl_c().await;
        println!("Stopping tunnel...");
    }
    
    Ok(())
}
