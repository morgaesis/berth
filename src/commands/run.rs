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
        ssh::start_tunnel(remote, &ports).await?;
    }

    let remote_path = format!("~/berth/projects/{}", name);
    // Use nohup and disown to keep process alive after SSH closes
    let full_cmd = format!(
        "cd {} && nohup {} >/dev/null 2>&1 & disown",
        remote_path, cmd_str
    );

    println!("Running on {}: cd {} && {}", remote, remote_path, cmd_str);
    
    let output = ssh::run_remote_command(remote, &full_cmd).await?;
    
    if !output.is_empty() {
        println!("{}", output);
    }
    
    if !ports.is_empty() {
        println!("Tunnel active for port(s): {:?} -> http://localhost:{}", ports, ports[0]);
    }
    
    Ok(())
}
