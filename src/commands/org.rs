use anyhow::Result;
use berth::config::{Config, Org};

pub async fn set(name: String, remote: Option<String>, root: Option<String>) -> Result<()> {
    let mut config = Config::load()?;
    let entry = config.orgs.entry(name.clone()).or_default();
    if let Some(r) = remote.as_ref() {
        entry.remote = Some(r.clone());
    }
    if let Some(r) = root.as_ref() {
        entry.remote_root = Some(r.clone());
    }
    config.save()?;
    println!("org '{}' updated:", name);
    print_org(&name, config.orgs.get(&name).unwrap());
    Ok(())
}

pub async fn remove(name: String) -> Result<()> {
    let mut config = Config::load()?;
    match config.orgs.remove(&name) {
        Some(_) => {
            config.save()?;
            println!("org '{name}' removed");
        }
        None => println!("org '{name}' not found"),
    }
    Ok(())
}

pub async fn list() -> Result<()> {
    let config = Config::load()?;
    if config.orgs.is_empty() {
        println!("(no orgs configured)");
        return Ok(());
    }
    let mut names: Vec<&String> = config.orgs.keys().collect();
    names.sort();
    for name in names {
        print_org(name, &config.orgs[name]);
    }
    Ok(())
}

pub async fn show(name: String) -> Result<()> {
    let config = Config::load()?;
    match config.orgs.get(&name) {
        Some(org) => {
            print_org(&name, org);
        }
        None => {
            anyhow::bail!("org '{name}' not configured; try `berth org set {name} …`");
        }
    }
    Ok(())
}

fn print_org(name: &str, org: &Org) {
    println!("  {name}:");
    println!(
        "    remote      = {}",
        org.remote.as_deref().unwrap_or("(none)")
    );
    println!(
        "    remote_root = {}",
        org.remote_root.as_deref().unwrap_or("(none)")
    );
}
