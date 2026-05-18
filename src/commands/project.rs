//! `berth project {show, set, list}` — per-workspace configuration via
//! the CLI, so users don't have to hand-edit `config.yaml`.

use anyhow::{bail, Result};
use berth::config::{Config, Workspace};
use colored::Colorize;

pub struct SetArgs {
    pub name: String,
    pub remote: Option<String>,
    pub clear_remote: bool,
    pub dir: Option<String>,
    pub clear_dir: bool,
    pub ports: Option<Vec<u16>>,
    pub clear_ports: bool,
    pub command: Vec<String>,
    pub clear_command: bool,
}

pub async fn show(name: String) -> Result<()> {
    let config = Config::load()?;
    let Some(ws) = config.workspaces.get(&name) else {
        bail!("workspace '{name}' not configured; create with `berth new {name}` first");
    };
    print_workspace(&config, &name, ws);
    Ok(())
}

pub async fn list_all() -> Result<()> {
    let config = Config::load()?;
    if config.workspaces.is_empty() {
        println!("(no workspaces configured)");
        return Ok(());
    }
    let mut names: Vec<&String> = config.workspaces.keys().collect();
    names.sort();
    for name in names {
        print_workspace(&config, name, &config.workspaces[name]);
        println!();
    }
    Ok(())
}

pub async fn set(args: SetArgs) -> Result<()> {
    let mut config = Config::load()?;
    let Some(ws) = config.workspaces.get_mut(&args.name) else {
        bail!(
            "workspace '{}' not configured; create with `berth new {}` first",
            args.name,
            args.name
        );
    };

    if args.clear_remote && args.remote.is_some() {
        bail!("--remote and --clear-remote are mutually exclusive");
    }
    if args.clear_dir && args.dir.is_some() {
        bail!("--dir and --clear-dir are mutually exclusive");
    }
    if args.clear_ports && args.ports.is_some() {
        bail!("--ports and --clear-ports are mutually exclusive");
    }
    if args.clear_command && !args.command.is_empty() {
        bail!("--clear-command and a trailing `-- <cmd>` are mutually exclusive");
    }

    let mut changed = false;
    if let Some(r) = args.remote {
        berth::validate_ssh_host(&r)?;
        ws.remote = Some(r);
        changed = true;
    } else if args.clear_remote {
        ws.remote = None;
        changed = true;
    }
    if let Some(d) = args.dir {
        ws.remote_dir = Some(d);
        changed = true;
    } else if args.clear_dir {
        ws.remote_dir = None;
        changed = true;
    }
    if let Some(p) = args.ports {
        ws.ports = if p.is_empty() { None } else { Some(p) };
        changed = true;
    } else if args.clear_ports {
        ws.ports = None;
        changed = true;
    }
    if !args.command.is_empty() {
        ws.command = Some(args.command);
        changed = true;
    } else if args.clear_command {
        ws.command = None;
        changed = true;
    }

    if !changed {
        bail!(
            "nothing to change; pass --dir, --remote, --ports, --clear-*, or a trailing `-- <cmd>`"
        );
    }
    config.save()?;
    eprintln!("{} updated", "✓".green().bold());
    print_workspace(&config, &args.name, &config.workspaces[&args.name]);
    Ok(())
}

fn print_workspace(config: &Config, name: &str, ws: &Workspace) {
    println!("{}", name.bold());
    let host = config
        .resolved_remote(name, ws)
        .unwrap_or_else(|| "(local)".to_string());
    let dir = config
        .resolved_remote_dir(name, ws)
        .unwrap_or_else(|| "(auto-managed)".to_string());
    println!(
        "  host       = {}",
        colored_value(&host, ws.remote.is_some())
    );
    println!(
        "  dir        = {}",
        colored_value(&dir, ws.remote_dir.is_some())
    );
    if let Some(ports) = &ws.ports {
        let joined = ports
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        println!("  ports      = {}", joined.cyan());
    } else {
        println!("  ports      = {}", "(none)".dimmed());
    }
    match &ws.command {
        Some(argv) => println!("  command    = {}", argv.join(" ").cyan()),
        None => println!("  command    = {}  (default $SHELL -l)", "(none)".dimmed()),
    }
    println!("  local path = {}", ws.path.dimmed());
}

/// Color cyan when the value is set on the workspace itself; dim when it
/// comes from an org default or auto-management. Makes the "what's
/// inherited?" question visible at a glance.
fn colored_value(value: &str, owned_by_workspace: bool) -> String {
    if owned_by_workspace {
        value.cyan().to_string()
    } else {
        format!("{value} {}", "(inherited)".dimmed())
    }
}
