use anyhow::{bail, Result};
use berth::config::{Config, Workspace};
use std::env;
use std::fs;
use std::path::PathBuf;

fn default_projects_path() -> std::path::PathBuf {
    if let Ok(dir) = env::var("BERTH_DATA_DIR") {
        return std::path::PathBuf::from(dir).join("projects");
    }
    if let Ok(dir) = env::var("XDG_DATA_HOME") {
        return std::path::PathBuf::from(dir).join("berth").join("projects");
    }

    dirs::data_local_dir()
        .map(|p| p.join("berth").join("projects"))
        .unwrap_or_else(|| std::path::PathBuf::from("~/.local/share/berth/projects"))
}

pub struct NewArgs {
    pub name: String,
    pub path: Option<String>,
    pub remote: Option<String>,
    pub ports: Vec<u16>,
    pub remote_dir: Option<String>,
    pub command: Vec<String>,
}

pub async fn run(args: NewArgs) -> Result<()> {
    let NewArgs {
        name,
        path,
        remote,
        ports,
        remote_dir,
        command,
    } = args;
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

    let mut workspace = Workspace::new(workspace_path.to_string_lossy().to_string());
    workspace.remote = remote;
    workspace.ports = if ports.is_empty() { None } else { Some(ports) };
    workspace.remote_dir = remote_dir;
    workspace.command = if command.is_empty() {
        None
    } else {
        Some(command)
    };

    config.workspaces.insert(name.clone(), workspace);
    config.save()?;

    println!(
        "Created workspace '{}' at {}",
        name,
        workspace_path.display()
    );
    Ok(())
}
