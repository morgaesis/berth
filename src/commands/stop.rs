use berth::config::Config;
use anyhow::{bail, Result};

pub async fn run(name: String) -> Result<()> {
    let config = Config::load()?;
    
    if !config.workspaces.contains_key(&name) {
        bail!("Workspace '{}' not found", name);
    }

    println!("Workspace '{}' is local. Use 'exit' to leave.", name);
    println!("For remote workspaces, this would stop the remote agent.");

    Ok(())
}
