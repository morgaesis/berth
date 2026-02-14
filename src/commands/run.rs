use berth::config::Config;
use berth::ssh;
use anyhow::Result;

pub async fn run(name: String, command: Vec<String>) -> Result<()> {
    let config = Config::load()?;
    
    let workspace = config.workspaces.get(&name)
        .ok_or_else(|| anyhow::anyhow!("Workspace '{}' not found", name))?;

    let remote = workspace.remote.as_ref()
        .ok_or_else(|| anyhow::anyhow!("Workspace '{}' is not remote", name))?;

    let cmd_str = command.join(" ");
    if cmd_str.is_empty() {
        anyhow::bail!("No command specified");
    }

    let remote_path = format!("~/berth/projects/{}", name);
    let full_cmd = format!("cd {} && {}", remote_path, cmd_str);

    println!("Running on {}: {}", remote, full_cmd);
    
    let output = ssh::run_remote_command(remote, &full_cmd).await?;
    
    println!("{}", output);
    
    Ok(())
}
