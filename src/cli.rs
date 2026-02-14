use crate::commands;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "berth", about = "Consistent development workspaces, local or remote, bare metal")]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    
    #[arg(help = "Workspace name (org/project format allowed). Creates workspace if it doesn't exist.")]
    name: Option<String>,
    
    #[arg(short = 'r', long = "remote", help = "SSH connection string (user@host) for remote workspace")]
    remote: Option<String>,
    
    #[arg(short = 'p', long = "ports", help = "Comma-separated list of ports to forward from remote", value_delimiter = ',')]
    ports: Vec<u16>,
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
        #[arg(short = 'p', long = "ports", help = "Start tunnel for these ports (requires remote)", value_delimiter = ',')]
        ports: Vec<u16>,
        #[arg(trailing_var_arg = true)]
        command: Vec<String>,
    },
    #[command(about = "Print shell integration script")]
    InitShell,
    #[command(about = "Run berth agent on remote machine")]
    Agent {
        #[arg(short = 'p', long = "ports", value_delimiter = ',')]
        ports: Vec<u16>,
    },
    #[command(subcommand, name = "hosts")]
    Hosts(HostsCommands),
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
                Commands::New { name, path, remote, ports } => {
                    berth::validate_workspace_name(&name)?;
                    commands::new::run(name, path, remote, ports).await
                }
                Commands::Enter { name, remote, ports } => {
                    berth::validate_workspace_name(&name)?;
                    commands::enter::run(name, remote, ports).await
                }
                Commands::List => {
                    commands::list::run().await
                }
                Commands::Tunnel { name, ports } => {
                    berth::validate_workspace_name(&name)?;
                    commands::tunnel::run(name, ports).await
                }
                Commands::Stop { name } => {
                    berth::validate_workspace_name(&name)?;
                    commands::stop::run(name).await
                }
                Commands::Delete { name } => {
                    berth::validate_workspace_name(&name)?;
                    commands::delete::run(name).await
                }
                Commands::Run { name, command, ports, remote } => {
                    berth::validate_workspace_name(&name)?;
                    commands::run::run(name, command, ports, remote).await
                }
                Commands::InitShell => {
                    commands::init_shell::run()
                }
                Commands::Agent { ports } => {
                    commands::agent::run(ports).await
                }
                Commands::Hosts(command) => match command {
                    HostsCommands::Update => commands::hosts::update().await,
                    HostsCommands::Clean => commands::hosts::clean().await,
                    HostsCommands::Install => commands::hosts::install().await,
                },
            }
        } else if let Some(name) = self.name {
            berth::validate_workspace_name(&name)?;
            commands::enter::run(name, self.remote, self.ports).await
        } else {
            commands::list::run().await
        }
    }
}
