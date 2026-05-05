use anyhow::Result;
use berth::config::Config;
use berth::hosts;

pub async fn update() -> Result<()> {
    let config = Config::load()?;

    hosts::clean()?;

    for name in config.workspaces.keys() {
        hosts::add_entry(name)?;
    }

    println!(
        "Updated /etc/hosts with {} workspace(s)",
        config.workspaces.len()
    );
    Ok(())
}

pub async fn clean() -> Result<()> {
    hosts::clean()?;
    println!("Cleaned berth entries from /etc/hosts");
    Ok(())
}

pub async fn install() -> Result<()> {
    hosts::install()
}
