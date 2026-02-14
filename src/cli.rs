use crate::commands;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "berth")]
#[command(about = "Consistent development workspaces, local or remote, bare metal")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    New {
        name: String,
        path: Option<String>,
        #[arg(short, long)]
        remote: Option<String>,
        #[arg(short, long, value_delimiter = ',')]
        ports: Vec<u16>,
    },
    Enter {
        name: String,
        #[arg(short, long)]
        remote: Option<String>,
    },
    List,
    Tunnel {
        name: String,
        #[arg(short, long, value_delimiter = ',')]
        ports: Vec<u16>,
    },
    Stop {
        name: String,
    },
    Delete {
        name: String,
    },
    InitShell,
    Agent {
        #[arg(short, long)]
        ports: Vec<u16>,
    },
    Hosts {
        #[command(subcommand)]
        command: HostsCommands,
    },
}

#[derive(Subcommand)]
enum HostsCommands {
    Update,
    Clean,
}

impl Cli {
    pub async fn run(self) -> anyhow::Result<()> {
        match self.command {
            Commands::New { name, path, remote, ports } => {
                commands::new::run(name, path, remote, ports).await
            }
            Commands::Enter { name, remote } => {
                commands::enter::run(name, remote).await
            }
            Commands::List => {
                commands::list::run().await
            }
            Commands::Tunnel { name, ports } => {
                commands::tunnel::run(name, ports).await
            }
            Commands::Stop { name } => {
                commands::stop::run(name).await
            }
            Commands::Delete { name } => {
                commands::delete::run(name).await
            }
            Commands::InitShell => {
                commands::init_shell::run()
            }
            Commands::Agent { ports } => {
                commands::agent::run(ports).await
            }
            Commands::Hosts { command } => match command {
                HostsCommands::Update => commands::hosts::update().await,
                HostsCommands::Clean => commands::hosts::clean().await,
            },
        }
    }
}
