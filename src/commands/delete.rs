use berth::config::Config;
use berth::hosts;
use anyhow::{bail, Result};

pub async fn run(name: String) -> Result<()> {
    let mut config = Config::load()?;
    
    if !config.workspaces.contains_key(&name) {
        bail!("Workspace '{}' not found", name);
    }

    config.workspaces.remove(&name);
    config.save()?;

    hosts::remove_entry(&name)?;

    println!("Deleted workspace '{}'", name);
    Ok(())
}
