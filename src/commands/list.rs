use anyhow::Result;
use berth::config::Config;

pub async fn run() -> Result<()> {
    let config = Config::load()?;

    if config.workspaces.is_empty() {
        println!("No workspaces configured.");
        return Ok(());
    }

    println!("{:<20} {:<10} {}", "NAME", "TYPE", "PATH");
    println!("{}", "-".repeat(60));

    for (name, ws) in &config.workspaces {
        let ws_type = if ws.remote.is_some() {
            "remote"
        } else {
            "local"
        };
        println!("{:<20} {:<10} {}", name, ws_type, ws.path);
    }

    Ok(())
}
