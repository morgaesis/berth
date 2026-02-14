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
    let tunnel_active = if !ports.is_empty() {
        ssh::start_tunnel(remote, &name, &ports).await?
    } else {
        false
    };

    let remote_path = format!("~/berth/projects/{}", name);
    let full_cmd = format!(
        "cd {} && nohup {} >/dev/null 2>&1 & disown",
        remote_path, cmd_str
    );

    println!("Running on {}: cd {} && {}", remote, remote_path, cmd_str);
    
    let output = ssh::run_remote_command(remote, &full_cmd).await?;
    
    if !output.is_empty() {
        println!("{}", output);
    } else {
        println!("Command started successfully.");
    }
    
    if tunnel_active && !ports.is_empty() {
        println!("Tunnel active: http://localhost:{}", ports[0]);
    }
    
    Ok(())
}
