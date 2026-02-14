use berth::config::Config;
use anyhow::{bail, Result};

pub async fn run(name: String) -> Result<()> {
    let mut config = Config::load()?;
    
    if !config.workspaces.contains_key(&name) {
        bail!("Workspace '{}' not found", name);
    }

    config.workspaces.remove(&name);
    config.save()?;

    println!("Deleted workspace '{}'", name);
    Ok(())
}
