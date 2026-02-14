use berth::config::{Config, Workspace};
use anyhow::{bail, Result};
use std::path::PathBuf;
use std::fs;

fn default_projects_path() -> std::path::PathBuf {
    dirs::data_local_dir()
        .map(|p| p.join("berth").join("projects"))
        .unwrap_or_else(|| std::path::PathBuf::from("~/.local/share/berth/projects"))
}

pub async fn run(name: String, path: Option<String>, remote: Option<String>, ports: Vec<u16>) -> Result<()> {
    let mut config = Config::load()?;
    
    if config.workspaces.contains_key(&name) {
        bail!("Workspace '{}' already exists", name);
    }

    let workspace_path = match path {
        Some(p) => PathBuf::from(p),
        None => default_projects_path().join(&name),
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
