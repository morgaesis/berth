use berth::config::Config;
use berth::ssh;
use anyhow::Result;

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

    // Get the remote host
    let host = remote.as_ref()
        .ok_or_else(|| anyhow::anyhow!("No remote configured for workspace '{}'. Use --remote or set remote in workspace.", name))?;

    let cmd_str = command.join(" ");
    if cmd_str.is_empty() {
        anyhow::bail!("No command specified");
    }

    // Start tunnel first if ports specified
    let tunnel_active = if !ports.is_empty() {
        ssh::start_tunnel(host, &name, &ports).await?
    } else {
        false
    };

    let remote_path = format!("~/berth/projects/{}", name);
    let full_cmd = format!(
        "cd {} && nohup {} >/dev/null 2>&1 & disown",
        remote_path, cmd_str
    );

    println!("Running on {}: cd {} && {}", host, remote_path, cmd_str);
    
    let output = ssh::run_remote_command(host, &full_cmd).await?;
    
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
