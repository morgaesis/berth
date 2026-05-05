use anyhow::{bail, Result};
use berth::config::Config;
use berth::ssh;

pub async fn run(name: String, ports: Vec<u16>) -> Result<()> {
    let config = Config::load()?;

    let workspace = config
        .workspaces
        .get(&name)
        .ok_or_else(|| anyhow::anyhow!("Workspace '{}' not found", name))?;

    let host = workspace
        .remote
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Workspace '{}' is not remote", name))?;

    if ports.is_empty() && workspace.ports.is_none() {
        bail!("No ports specified. Use --ports or configure ports in workspace.");
    }

    let ports_to_forward = if ports.is_empty() {
        workspace.ports.as_ref().unwrap().clone()
    } else {
        ports
    };

    println!("Tunneling ports {:?} from {}...", ports_to_forward, host);
    println!("Press Ctrl+C to stop.\n");

    ssh::start_tunnel(host, &name, &ports_to_forward).await?;

    Ok(())
}
