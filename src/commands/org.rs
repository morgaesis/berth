use anyhow::Result;
use berth::config::{Config, Org};

pub async fn set(
    name: String,
    remote: Option<String>,
    root: Option<String>,
    user: Option<String>,
) -> Result<()> {
    let mut config = Config::load()?;
    let entry = config.orgs.entry(name.clone()).or_default();
    if let Some(r) = remote.as_ref() {
        entry.remote = Some(r.clone());
    }
    if let Some(r) = root.as_ref() {
        entry.remote_root = Some(r.clone());
    }
    if let Some(u) = user.as_ref() {
        entry.remote_user = Some(u.clone());
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
            print_workspaces_for_org(&config, &name);
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
    println!(
        "    remote_user = {}",
        org.remote_user.as_deref().unwrap_or("(none)")
    );
}

fn print_workspaces_for_org(config: &Config, org: &str) {
    let prefix = format!("{org}/");
    let mut names: Vec<&String> = config
        .workspaces
        .keys()
        .filter(|n| n.starts_with(&prefix))
        .collect();
    names.sort();
    if names.is_empty() {
        println!("    workspaces  = (none)");
        return;
    }
    println!("    workspaces:");
    for ws_name in names {
        let ws = &config.workspaces[ws_name];
        let dir = config
            .resolved_remote_dir(ws_name, ws)
            .unwrap_or_else(|| "(auto-managed)".to_string());
        let host = config
            .resolved_remote(ws_name, ws)
            .unwrap_or_else(|| "(local)".to_string());
        let cmd_summary = ws
            .command
            .as_ref()
            .map(|argv| {
                let joined: String = argv.join(" ");
                if joined.chars().count() > 60 {
                    let truncated: String = joined.chars().take(57).collect();
                    format!(" cmd=`{truncated}…`")
                } else {
                    format!(" cmd=`{joined}`")
                }
            })
            .unwrap_or_default();
        println!("      - {ws_name}");
        println!("          host = {host}");
        println!("          dir  = {dir}{cmd_summary}");
    }
}
