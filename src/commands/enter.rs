use berth::config::Config;
use berth::hosts;
use berth::ssh;
use anyhow::{bail, Result};
use std::env;
use std::path::Path;

pub async fn run(name: String, remote_override: Option<String>) -> Result<()> {
    let config = Config::load()?;
    
    let workspace = config.workspaces.get(&name)
        .ok_or_else(|| anyhow::anyhow!("Workspace '{}' not found", name))?;

    let path = Path::new(&workspace.path);
    if !path.exists() {
        bail!("Workspace path does not exist: {}", workspace.path);
    }

    let remote = remote_override.as_ref().or(workspace.remote.as_ref());

    if let Some(host) = remote {
        enter_remote(name, host, path, workspace.ports.as_ref()).await
    } else {
        enter_local(name, path)
    }
}

fn enter_local(name: String, path: &Path) -> Result<()> {
    hosts::add_entry(&name)?;
    
    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    
    println!("\x1b]2;berth: {}\x07", name);
    
    let mut child = std::process::Command::new(&shell)
        .current_dir(path)
        .env("BERTH_WORKSPACE", &name)
        .env("BERTH_PATH", path)
        .spawn()?;
    
    child.wait()?;
    Ok(())
}

async fn enter_remote(name: String, host: &str, path: &Path, ports: Option<&Vec<u16>>) -> Result<()> {
    hosts::add_entry(&name)?;
    
    if let Some(ports) = ports {
        let _tunnel = ssh::start_tunnel(host, ports).await?;
    }

    println!("\x1b]2;berth: {} [{}]\x07", name, host);
    
    ssh::ssh_interactive(host, path).await?;

    Ok(())
}
