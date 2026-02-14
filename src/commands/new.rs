use berth::config::{Config, Workspace};
use anyhow::{bail, Result};
use std::path::PathBuf;
use std::fs;

pub async fn run(name: String, path: Option<String>, remote: Option<String>, ports: Vec<u16>) -> Result<()> {
    let mut config = Config::load()?;
    
    if config.workspaces.contains_key(&name) {
        bail!("Workspace '{}' already exists", name);
    }

    let workspace_path = match path {
        Some(p) => PathBuf::from(p),
        None => dirs::home_dir()
            .map(|h| h.join("projects").join(&name))
            .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?,
    };

    if !workspace_path.exists() {
        fs::create_dir_all(&workspace_path)?;
    }

    let workspace = Workspace {
        path: workspace_path.to_string_lossy().to_string(),
        remote,
        ports: if ports.is_empty() { None } else { Some(ports) },
    };

    config.workspaces.insert(name.clone(), workspace);
    config.save()?;

    println!("Created workspace '{}' at {}", name, workspace_path.display());
    Ok(())
}
