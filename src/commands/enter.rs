use berth::config::{Config, Workspace};
use berth::ssh;
use anyhow::Result;
use std::env;
use std::fs;
use std::path::Path;

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

pub async fn run(name: String, remote_override: Option<String>, ports_override: Vec<u16>) -> Result<()> {
    let mut config = Config::load()?;
    
    let workspace = if let Some(ws) = config.workspaces.get(&name) {
        ws.clone()
    } else {
        let default_path = default_projects_path().join(&name);
        
        let path_str = default_path.to_string_lossy().to_string();
        
        if !default_path.exists() {
            fs::create_dir_all(&default_path)?;
            println!("Created directory: {}", path_str);
        }
        
        let workspace = Workspace {
            path: path_str.clone(),
            remote: remote_override.clone(),
            ports: if ports_override.is_empty() { None } else { Some(ports_override.clone()) },
        };
        
        config.workspaces.insert(name.clone(), workspace.clone());
        config.save()?;
        println!("Created workspace '{}' at {}", name, path_str);
        
        workspace
    };

    let path = Path::new(&workspace.path);
    if !path.exists() {
        fs::create_dir_all(path)?;
    }

    let remote = remote_override.as_ref().or(workspace.remote.as_ref());
    let ports = if !ports_override.is_empty() { 
        Some(ports_override.as_slice())
    } else { 
        workspace.ports.as_deref()
    };

    if let Some(host) = remote {
        enter_remote(name, host, path, ports).await
    } else {
        enter_local(name, path)
    }
}

fn enter_local(name: String, path: &Path) -> Result<()> {
    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    
    println!("\x1b]2;berth: {}\x07", name);
    
    let mut child = std::process::Command::new(&shell)
        .current_dir(path)
        .env("BERTH_WORKSPACE", &name)
        .env("BERTH_PATH", path.to_string_lossy().as_ref())
        .spawn()?;
    
    child.wait()?;
    Ok(())
}

async fn enter_remote(name: String, host: &str, _path: &Path, ports: Option<&[u16]>) -> Result<()> {
    if let Some(ports) = ports {
        let _tunnel = ssh::start_tunnel(host, &name, ports).await?;
    }

    println!("\x1b]2;berth: {} [{}]\x07", name, host);
    
    ssh::ssh_interactive(host, &name, true).await?;

    Ok(())
}
