use crate::commands;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "berth",
    about = "Consistent development workspaces, local or remote, bare metal",
    after_help = "Shell niceties and aliases: eval \"$(berth init-shell)\""
)]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Create a new workspace configuration")]
    New {
        #[arg(help = "Workspace name (org/project format allowed)")]
        name: String,
        path: Option<String>,
        #[arg(short = 'r', long = "remote")]
        remote: Option<String>,
        #[arg(short = 'p', long = "ports", value_delimiter = ',')]
        ports: Vec<u16>,
    },
    #[command(about = "Enter a workspace (creates if needed)")]
    Enter {
        #[arg(help = "Workspace name (org/project format allowed)")]
        name: String,
        #[arg(short = 'r', long = "remote")]
        remote: Option<String>,
        #[arg(short = 'p', long = "ports", value_delimiter = ',')]
        ports: Vec<u16>,
    },
    #[command(about = "List all configured workspaces")]
    List,
    #[command(about = "Tunnel remote ports locally")]
    Tunnel {
        #[arg(help = "Workspace name (org/project format allowed)")]
        name: String,
        #[arg(short = 'p', long = "ports", value_delimiter = ',')]
        ports: Vec<u16>,
    },
    #[command(about = "Stop a workspace")]
    Stop {
        #[arg(help = "Workspace name (org/project format allowed)")]
        name: String,
    },
    #[command(about = "Stop expired local container environments")]
    Reap,
    #[command(about = "Run the local Berth daemon in the foreground")]
    Daemon {
        #[arg(
            long = "interval-seconds",
            help = "Seconds between idle reaper runs",
            default_value_t = 300
        )]
        interval_seconds: u64,
        #[arg(
            long = "once",
            help = "Run one daemon iteration and exit; useful for tests and external supervisors"
        )]
        once: bool,
    },
    #[command(about = "Show local runtime auto-discovery status")]
    Doctor,
    #[command(about = "Delete a workspace configuration")]
    Delete {
        #[arg(help = "Workspace name (org/project format allowed)")]
        name: String,
    },
    #[command(about = "Run a command on a workspace")]
    Run {
        #[arg(help = "Workspace name (org/project format allowed)")]
        name: String,
        #[arg(short = 'r', long = "remote", help = "Override remote SSH host")]
        remote: Option<String>,
        #[arg(
            short = 'p',
            long = "ports",
            help = "Start tunnel for these ports (requires remote)",
            value_delimiter = ','
        )]
        ports: Vec<u16>,
        #[arg(trailing_var_arg = true)]
        command: Vec<String>,
    },
    #[command(
        about = "Print shell integration script",
        after_help = "Install shell niceties and aliases with: eval \"$(berth init-shell)\""
    )]
    InitShell,
    #[command(about = "Run berth agent on remote machine")]
    Agent {
        #[arg(short = 'p', long = "ports", value_delimiter = ',')]
        ports: Vec<u16>,
    },
    #[command(subcommand, name = "hosts")]
    Hosts(HostsCommands),
    #[command(external_subcommand)]
    External(Vec<String>),
}

#[derive(Subcommand)]
enum HostsCommands {
    #[command(about = "Update hosts file with all workspace names")]
    Update,
    #[command(about = "Remove all berth entries from hosts file")]
    Clean,
    #[command(about = "Add wildcard *.berth entry to hosts file (requires sudo)")]
    Install,
}

impl Cli {
    pub async fn run(self) -> anyhow::Result<()> {
        if let Some(cmd) = self.command {
            match cmd {
                Commands::New {
                    name,
                    path,
                    remote,
                    ports,
                } => {
                    berth::validate_workspace_name(&name)?;
                    commands::new::run(name, path, remote, ports).await
                }
                Commands::Enter {
                    name,
                    remote,
                    ports,
                } => {
                    berth::validate_workspace_name(&name)?;
                    commands::enter::run(name, remote, ports).await
                }
                Commands::List => commands::list::run().await,
                Commands::Tunnel { name, ports } => {
                    berth::validate_workspace_name(&name)?;
                    commands::tunnel::run(name, ports).await
                }
                Commands::Stop { name } => {
                    berth::validate_workspace_name(&name)?;
                    commands::stop::run(name).await
                }
                Commands::Reap => commands::reap::run().await,
                Commands::Daemon {
                    interval_seconds,
                    once,
                } => commands::daemon::run(Some(interval_seconds), once).await,
                Commands::Doctor => commands::doctor::run().await,
                Commands::Delete { name } => {
                    berth::validate_workspace_name(&name)?;
                    commands::delete::run(name).await
                }
                Commands::Run {
                    name,
                    command,
                    ports,
                    remote,
                } => {
                    berth::validate_workspace_name(&name)?;
                    commands::run::run(name, command, ports, remote).await
                }
                Commands::InitShell => commands::init_shell::run(),
                Commands::Agent { ports } => commands::agent::run(ports).await,
                Commands::Hosts(command) => match command {
                    HostsCommands::Update => commands::hosts::update().await,
                    HostsCommands::Clean => commands::hosts::clean().await,
                    HostsCommands::Install => commands::hosts::install().await,
                },
                Commands::External(args) => {
                    let name = args.first().map(String::as_str).unwrap_or("NAME");
                    anyhow::bail!(
                        "implicit workspace shorthand was removed; use `berth enter {name}` instead, or install shell helpers with `eval \"$(berth init-shell)\"` and use `b {name}`"
                    )
                }
            }
        } else {
            commands::list::run().await
        }
    }
}
