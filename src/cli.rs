use crate::commands;
use crate::commands::shell::HookShell;
use clap::{Parser, Subcommand};
use clap_complete::Shell as CompletionShell;

#[derive(Parser)]
#[command(
    name = "berth",
    version,
    about = "Consistent development workspaces, local or remote, bare metal",
    after_help = "Shell niceties: eval \"$(berth shell-init)\"   Completions: berth shell-completions"
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
    #[command(
        about = "Enter a workspace (creates if needed)",
        long_about = "Enter a workspace, creating it if absent.\n\n\
                      For remote workspaces, berth probes the host and selects the best\n\
                      session-mux available. If none, you'll be prompted to deploy the\n\
                      berth binary to the remote (one-time consent, persisted in config).\n\
                      \n\
                      Flags:\n  \
                      --plain         skip session-mux entirely; plain SSH login shell\n  \
                      --auto-deploy   deploy without prompting (overrides per-host trust)\n  \
                      --no-deploy     never deploy; fall through to legacy multiplexers\n  \
                                      or fail with a `--plain` suggestion"
    )]
    Enter {
        #[arg(help = "Workspace name (org/project format allowed)")]
        name: String,
        #[arg(short = 'r', long = "remote")]
        remote: Option<String>,
        #[arg(short = 'p', long = "ports", value_delimiter = ',')]
        ports: Vec<u16>,
        #[arg(
            long = "plain",
            alias = "no-resume",
            help = "Skip session-mux; just open a plain SSH login shell"
        )]
        plain: bool,
        #[arg(
            long = "auto-deploy",
            conflicts_with_all = ["plain", "no_deploy"],
            help = "Deploy berth binary to the remote without prompting"
        )]
        auto_deploy: bool,
        #[arg(
            long = "no-deploy",
            conflicts_with_all = ["plain", "auto_deploy"],
            help = "Never deploy; use legacy multiplexers or fail"
        )]
        no_deploy: bool,
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
        about = "Print shell integration script (deprecated alias of shell-init)",
        long_about = "Deprecated alias of `berth shell-init`. Will be removed in a future release; \
                      switch invocations to `eval \"$(berth shell-init)\"`.",
        after_help = "Install shell niceties and aliases with: eval \"$(berth shell-init)\""
    )]
    InitShell,
    #[command(
        name = "shell-init",
        about = "Print shell hook for resumable session auto-entry",
        long_about = "Print a shell init script that auto-enters a berth workspace in new tabs.\n\
                      Cascades detection: WezTerm user var → OSC 7 inherited PWD marker → no hook.\n\
                      Install: eval \"$(berth shell-init)\""
    )]
    ShellInit {
        #[arg(
            value_enum,
            help = "Target shell (auto-detected from $SHELL when omitted)"
        )]
        shell: Option<HookShell>,
    },
    #[command(
        name = "shell-completions",
        about = "Print shell completion script",
        long_about = "Emit completion script for the given shell. Auto-detects from $SHELL when omitted.\n\
                      Install (zsh): berth shell-completions zsh > ~/.zsh/completions/_berth\n\
                      Install (bash): berth shell-completions bash > ~/.local/share/bash-completion/completions/berth"
    )]
    ShellCompletions {
        #[arg(
            value_enum,
            help = "Target shell (auto-detected from $SHELL when omitted)"
        )]
        shell: Option<CompletionShell>,
    },
    #[command(about = "Run berth agent on remote machine")]
    Agent {
        #[arg(short = 'p', long = "ports", value_delimiter = ',')]
        ports: Vec<u16>,
    },
    #[command(
        about = "Attach to or start a resumable workspace session",
        long_about = "Resume a workspace session managed by the local berth supervisor.\n\n\
                      By default, attaches to the single existing session for the workspace.\n\
                      With --new, always starts a fresh independent session (used by the remote\n\
                      bootstrap of `berth enter` so each terminal tab gets its own PTY).\n\
                      With --session <id>, targets a specific session by id."
    )]
    Attach {
        #[arg(help = "Workspace name (org/project format allowed)")]
        name: String,
        #[arg(
            long = "new",
            help = "Start a fresh independent session instead of resuming"
        )]
        new: bool,
        #[arg(
            long = "session",
            value_name = "ID",
            help = "Attach to a specific session id (see `berth attach --list`)"
        )]
        session: Option<String>,
        #[arg(
            long = "list",
            help = "List active sessions for the workspace and exit"
        )]
        list: bool,
        #[arg(
            long = "supervisor",
            help = "Internal: run as the session supervisor in the foreground"
        )]
        supervisor: bool,
        #[arg(
            trailing_var_arg = true,
            help = "Override session command (defaults to login shell)"
        )]
        command: Vec<String>,
    },
    #[command(
        about = "Deploy the berth binary to a remote host over SSH",
        long_about = "Probe the remote host for OS+architecture, fetch the matching\n\
                      pre-built berth binary from this project's GitHub releases (verifying\n\
                      SHA256), and scp it to ~/.local/bin/berth on the remote.\n\
                      \n\
                      Subsequent `berth enter --remote <host>` invocations will then run\n\
                      `berth attach --new <ws>` on the far side for full per-tab independent\n\
                      sessions and resume support.\n\
                      \n\
                      Adds the host to `trusted_hosts` in the config on success so future\n\
                      enters auto-deploy without prompting when the remote binary is stale\n\
                      or missing."
    )]
    Deploy {
        #[arg(help = "SSH host (anything `ssh <host>` would accept)")]
        host: String,
        #[arg(
            long = "tag",
            help = "GitHub release tag to fetch (defaults to v<this-binary-version>)"
        )]
        tag: Option<String>,
        #[arg(long = "force", help = "Redeploy even if the remote binary matches")]
        force: bool,
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
                    if let Some(host) = &remote {
                        berth::validate_ssh_host(host)?;
                    }
                    commands::new::run(name, path, remote, ports).await
                }
                Commands::Enter {
                    name,
                    remote,
                    ports,
                    plain,
                    auto_deploy,
                    no_deploy,
                } => {
                    berth::validate_workspace_name(&name)?;
                    if let Some(host) = &remote {
                        berth::validate_ssh_host(host)?;
                    }
                    let opts = commands::enter::EnterOptions {
                        plain,
                        auto_deploy,
                        no_deploy,
                    };
                    commands::enter::run(name, remote, ports, opts).await
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
                    if let Some(host) = &remote {
                        berth::validate_ssh_host(host)?;
                    }
                    commands::run::run(name, command, ports, remote).await
                }
                Commands::InitShell => {
                    eprintln!("berth: `init-shell` is deprecated; switch to `shell-init`.");
                    commands::shell::run_init(None)
                }
                Commands::ShellInit { shell } => commands::shell::run_init(shell),
                Commands::ShellCompletions { shell } => commands::shell::run_completions(shell),
                Commands::Agent { ports } => commands::agent::run(ports).await,
                Commands::Attach {
                    name,
                    new,
                    session,
                    list,
                    supervisor,
                    command,
                } => {
                    berth::validate_workspace_name(&name)?;
                    let code = commands::attach::run(
                        name,
                        commands::attach::AttachOptions {
                            supervisor,
                            new,
                            session,
                            list,
                            command,
                        },
                    )
                    .await?;
                    if code != 0 {
                        std::process::exit(code);
                    }
                    Ok(())
                }
                Commands::Deploy { host, tag, force } => {
                    berth::validate_ssh_host(&host)?;
                    if let Some(t) = &tag {
                        berth::validate_release_tag(t)?;
                    }
                    commands::deploy::run(host, tag, force).await
                }
                Commands::Hosts(command) => match command {
                    HostsCommands::Update => commands::hosts::update().await,
                    HostsCommands::Clean => commands::hosts::clean().await,
                    HostsCommands::Install => commands::hosts::install().await,
                },
                Commands::External(args) => {
                    let name = args.first().map(String::as_str).unwrap_or("NAME");
                    anyhow::bail!(
                        "implicit workspace shorthand was removed; use `berth enter {name}` instead, or install shell helpers with `eval \"$(berth shell-init)\"` and use `b {name}`"
                    )
                }
            }
        } else {
            commands::list::run().await
        }
    }
}
