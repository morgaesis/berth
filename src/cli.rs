use crate::commands;
use crate::commands::shell::HookShell;
use clap::{Parser, Subcommand};
use clap_complete::Shell as CompletionShell;

#[derive(Parser)]
#[command(
    name = "berth",
    version,
    about = "Consistent development workspaces, local or remote, bare metal",
    after_help = "Tab completions: `berth shell completions <shell>`\n\
                  New-tab auto-entry: `eval \"$(berth shell init)\"` in your rc \
                  (see `berth doctor`)"
)]
pub struct Cli {
    /// Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides
    /// RUST_LOG when set; otherwise RUST_LOG is honored unchanged.
    #[arg(short = 'v', long = "verbose", global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Silence stderr log output entirely (overrides -v and RUST_LOG).
    #[arg(short = 'q', long = "quiet", global = true)]
    pub quiet: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

/// Combine a workspace `name` with an optional `--org` flag.
///
/// - If `name` is already `org/project` and `--org` is also supplied, error
///   when they disagree (both forms specifying different orgs is ambiguous).
/// - If `name` has no slash and `--org` is supplied, return `<org>/<name>`.
/// - Otherwise return `name` unchanged.
fn compose_workspace_name(name: &str, org: Option<&str>) -> anyhow::Result<String> {
    match (org, name.split_once('/')) {
        (None, _) => Ok(name.to_string()),
        (Some(o), None) => Ok(format!("{o}/{name}")),
        (Some(o), Some((existing_org, _))) if existing_org == o => Ok(name.to_string()),
        (Some(o), Some((existing_org, _))) => anyhow::bail!(
            "conflicting org: --org={o} but workspace name says {existing_org}/…; \
             pass one or the other, not both with different values"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_workspace_no_org_passes_through() {
        assert_eq!(compose_workspace_name("postil", None).unwrap(), "postil");
        assert_eq!(
            compose_workspace_name("morgaesis/postil", None).unwrap(),
            "morgaesis/postil"
        );
    }

    #[test]
    fn compose_workspace_prepends_org_to_bare_name() {
        assert_eq!(
            compose_workspace_name("postil", Some("morgaesis")).unwrap(),
            "morgaesis/postil"
        );
    }

    #[test]
    fn compose_workspace_org_matches_existing_prefix() {
        assert_eq!(
            compose_workspace_name("morgaesis/postil", Some("morgaesis")).unwrap(),
            "morgaesis/postil"
        );
    }

    #[test]
    fn compose_workspace_conflicting_org_errors() {
        assert!(compose_workspace_name("morgaesis/postil", Some("other")).is_err());
    }
}

impl Cli {
    /// Translate the verbosity flags into a tracing filter directive.
    /// Returns None to mean "honor whatever the environment sets".
    pub fn log_filter(&self) -> Option<&'static str> {
        if self.quiet {
            return Some("off");
        }
        match self.verbose {
            0 => None,
            1 => Some("berth=info"),
            2 => Some("berth=debug"),
            _ => Some("berth=trace"),
        }
    }
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "List configured workspaces (with last-used time)")]
    List {
        #[arg(
            short = 'l',
            long = "long",
            help = "Per-workspace resolved config block instead of the table"
        )]
        long: bool,
        #[arg(
            long = "abs",
            help = "Render last-used as absolute UTC timestamps (default: relative, e.g. `3d ago`)"
        )]
        abs: bool,
    },
    #[command(about = "Show one workspace's resolved config")]
    Show {
        #[arg(help = "Workspace name (org/project format allowed)")]
        name: String,
    },
    #[command(
        about = "Create a new workspace configuration",
        long_about = "Create a new workspace configuration.\n\n\
                      Workspaces can be plain (`postil`) or org-scoped (`morgaesis/postil`).\n\
                      Org-scoped workspaces inherit a remote host and remote-root directory\n\
                      from `orgs.<org>` in config (see `berth org set`).\n\n\
                      Examples:\n  \
                      berth new postil\n  \
                      berth new morgaesis/postil -r morgaesis-dev\n  \
                      berth new morgaesis/postil --dir '~/Projects/morgaesis/postil.dev'\n  \
                      berth new morgaesis/postil -- claude --dangerously-skip-permissions"
    )]
    New {
        #[arg(help = "Workspace name (org/project, or bare project paired with --org)")]
        name: String,
        #[arg(
            help = "Local path for the workspace (defaults to $XDG_DATA_HOME/berth/projects/<name>)"
        )]
        path: Option<String>,
        #[arg(
            short = 'o',
            long = "org",
            help = "Prepend this org to a bare workspace name (e.g. --org morgaesis postil → morgaesis/postil)"
        )]
        org: Option<String>,
        #[arg(short = 'r', long = "remote", help = "SSH host for remote entry")]
        remote: Option<String>,
        #[arg(short = 'p', long = "ports", value_delimiter = ',')]
        ports: Vec<u16>,
        #[arg(
            short = 'd',
            long = "dir",
            help = "Remote working directory (overrides the auto-managed or org-derived path)"
        )]
        dir: Option<String>,
        #[arg(
            trailing_var_arg = true,
            help = "Default command for `berth enter` (everything after `--`)"
        )]
        command: Vec<String>,
    },
    #[command(
        about = "Set or update fields on a workspace",
        long_about = "Update one or more fields on an existing workspace. \
                      Pair a `--<field>` flag with a value to set it, or \
                      `--clear-<field>` to unset and fall back to defaults. \
                      The trailing `-- <argv>` sets the command run on enter; \
                      `--clear-command` unsets it (returning to the default \
                      $SHELL -l)."
    )]
    Set {
        #[arg(help = "Workspace name")]
        name: String,
        #[arg(short = 'r', long = "remote", conflicts_with = "clear_remote")]
        remote: Option<String>,
        #[arg(long = "clear-remote", conflicts_with = "remote")]
        clear_remote: bool,
        #[arg(short = 'd', long = "dir", conflicts_with = "clear_dir")]
        dir: Option<String>,
        #[arg(long = "clear-dir", conflicts_with = "dir")]
        clear_dir: bool,
        #[arg(
            short = 'p',
            long = "ports",
            value_delimiter = ',',
            conflicts_with = "clear_ports"
        )]
        ports: Option<Vec<u16>>,
        #[arg(long = "clear-ports", conflicts_with = "ports")]
        clear_ports: bool,
        #[arg(long = "clear-command")]
        clear_command: bool,
        #[arg(trailing_var_arg = true)]
        command: Vec<String>,
    },
    #[command(about = "Delete a workspace configuration")]
    Rm {
        #[arg(help = "Workspace name (org/project format allowed)")]
        name: String,
    },
    #[command(
        about = "Enter a workspace (creates if needed)",
        long_about = "Enter a workspace, creating it if absent.\n\n\
                      Workspaces can be plain (`postil`) or org-scoped (`morgaesis/postil`).\n\
                      Use --org to compose an org with a bare project name. Org-scoped\n\
                      workspaces inherit a remote host and a remote-root directory from\n\
                      `orgs.<org>` in config (see `berth org set`).\n\n\
                      Examples:\n  \
                      berth enter postil --org morgaesis\n  \
                      berth enter morgaesis/postil --remote dev-box\n  \
                      berth enter morgaesis/postil --dir '~/Projects/morgaesis/postil'\n  \
                      berth enter morgaesis/postil -- claude --dangerously-skip-permissions\n\n\
                      For remote workspaces, berth probes the host and selects the best\n\
                      session-mux available. If none, you'll be prompted to deploy the\n\
                      berth binary to the remote (one-time consent, persisted in config).\n\n\
                      Resumability flags:\n  \
                      --plain         skip session-mux entirely; plain SSH login shell\n  \
                      --auto-deploy   deploy without prompting (overrides per-host trust)\n  \
                      --no-deploy     never deploy; fall through to legacy multiplexers\n\n\
                      New-tab replay: with the shell hook installed (see `berth doctor`),\n  \
                      new terminal tabs spawned from a berth session will re-run this same\n  \
                      invocation verbatim — including any trailing `-- <argv>` override. If\n  \
                      that command prompts interactively (e.g. sudo), the prompt will\n  \
                      reappear in each new tab. Set BERTH_SKIP_AUTO=1 to opt out for one\n  \
                      shell."
    )]
    Enter {
        #[arg(help = "Workspace name (org/project, or bare project paired with --org)")]
        name: String,
        #[arg(
            short = 'o',
            long = "org",
            help = "Prepend this org to the workspace name (e.g. --org morgaesis postil → morgaesis/postil)"
        )]
        org: Option<String>,
        #[arg(
            short = 'r',
            long = "remote",
            help = "SSH host (overrides workspace/org default)"
        )]
        remote: Option<String>,
        #[arg(short = 'p', long = "ports", value_delimiter = ',')]
        ports: Vec<u16>,
        #[arg(
            short = 'd',
            long = "dir",
            help = "Override the remote working directory (e.g. ~/code/postil)"
        )]
        dir: Option<String>,
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
        #[arg(
            long = "new",
            help = "Force a fresh session even if one is already alive (default: resume if a session exists)"
        )]
        new: bool,
        #[arg(
            long = "no-reconnect",
            help = "Disable the auto-reconnect loop on SSH-drop; bail on first connection loss"
        )]
        no_reconnect: bool,
        #[arg(
            trailing_var_arg = true,
            help = "Override workspace default command (everything after `--`)"
        )]
        command: Vec<String>,
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
            long = "resume-or-new",
            conflicts_with = "new",
            help = "Attach to an existing session if one exists, else create new (the verb `berth enter` invokes on the remote)"
        )]
        resume_or_new: bool,
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
    #[command(about = "Stop a workspace")]
    Stop {
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
    #[command(about = "Tunnel remote ports locally")]
    Tunnel {
        #[arg(help = "Workspace name (org/project format allowed)")]
        name: String,
        #[arg(short = 'p', long = "ports", value_delimiter = ',')]
        ports: Vec<u16>,
    },
    #[command(
        subcommand,
        name = "org",
        about = "Manage per-org defaults (remote host + remote-root path)",
        long_about = "Configure defaults for workspace names of the form `<org>/<project>`. \
                      A workspace can inherit its remote host and remote working-directory \
                      root from its org, so individual workspaces don't have to repeat the \
                      prefix.\n\n\
                      Examples:\n  \
                      berth org set morgaesis --remote morgaesis-dev --root '~/Projects/morgaesis'\n  \
                      berth org list\n  \
                      berth org show morgaesis"
    )]
    Org(OrgCommands),
    #[command(
        subcommand,
        name = "hosts",
        about = "Manage /etc/hosts entries for workspaces"
    )]
    Hosts(HostsCommands),
    #[command(
        subcommand,
        name = "shell",
        about = "Shell integration: init script (new-tab hook) + tab completions",
        long_about = "Generate the new-tab auto-entry hook and tab-completion scripts.\n\n\
                      Examples:\n  \
                      eval \"$(berth shell init)\"             # source the new-tab hook in your rc\n  \
                      eval \"$(berth shell completions)\"      # source completions in your rc\n  \
                      berth shell init bash > ~/.config/berth/init.sh\n  \
                      berth shell completions zsh > ~/.zsh/completions/_berth"
    )]
    Shell(ShellSubcommands),
    #[command(
        about = "Dump recent berth activity from local + supervisor logs",
        long_about = "Print the tail of the global berth log plus any per-session \
                      supervisor logs (which capture the PTY child's stdout+stderr — \
                      this is where a failed shell command's error text ends up).\n\n\
                      Useful for sharing state back to an AI agent or coworker when \
                      something hangs or errors unexpectedly."
    )]
    Logs {
        #[arg(short = 'n', long = "lines", help = "Tail length (default 200)")]
        lines: Option<usize>,
        #[arg(long = "follow", help = "Follow new log lines (not yet implemented)")]
        follow: bool,
        #[arg(
            long = "sessions",
            help = "Always include per-session supervisor logs even with -n"
        )]
        sessions: bool,
    },
    #[command(about = "Show shell-integration + local runtime status")]
    Doctor,
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
    #[command(about = "Stop expired local container environments")]
    Reap,
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
    #[command(about = "Run berth agent on remote machine (internal)")]
    Agent {
        #[arg(short = 'p', long = "ports", value_delimiter = ',')]
        ports: Vec<u16>,
    },
}

#[derive(Subcommand)]
enum ShellSubcommands {
    #[command(
        about = "Print the new-tab auto-entry hook script",
        long_about = "Print a shell init script. Source via `eval \"$(berth shell init)\"` \
                      in your bashrc/zshrc. The script hooks new shells so that, when opened \
                      from inside a berth workspace, they auto-re-enter the same workspace \
                      with the same command override."
    )]
    Init {
        #[arg(
            value_enum,
            help = "Target shell (auto-detected from $SHELL when omitted)"
        )]
        shell: Option<HookShell>,
    },
    #[command(
        about = "Print the completion script for the given shell",
        long_about = "Emit completion script for the given shell. Auto-detects from $SHELL \
                      when omitted.\n\n\
                      Install (zsh):  berth shell completions zsh  > ~/.zsh/completions/_berth\n\
                      Install (bash): berth shell completions bash > ~/.local/share/bash-completion/completions/berth"
    )]
    Completions {
        #[arg(
            value_enum,
            help = "Target shell (auto-detected from $SHELL when omitted)"
        )]
        shell: Option<CompletionShell>,
    },
}

#[derive(Subcommand)]
enum OrgCommands {
    #[command(about = "List all configured orgs")]
    List,
    #[command(about = "Show one org's defaults")]
    Show {
        #[arg(help = "Org name")]
        name: String,
    },
    #[command(about = "Set or update an org's defaults")]
    Set {
        #[arg(help = "Org name (e.g. morgaesis)")]
        name: String,
        #[arg(
            short = 'r',
            long = "remote",
            help = "Default SSH host for workspaces in this org"
        )]
        remote: Option<String>,
        #[arg(
            short = 'R',
            long = "root",
            help = "Default remote-root directory (final workspace dir = <root>/<project>)"
        )]
        root: Option<String>,
    },
    #[command(about = "Remove an org from config (doesn't touch any workspace)")]
    Rm {
        #[arg(help = "Org name")]
        name: String,
    },
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
                Commands::List { long, abs } => commands::list::run(long, abs).await,
                Commands::Show { name } => {
                    berth::validate_workspace_name(&name)?;
                    commands::project::show(name).await
                }
                Commands::New {
                    name,
                    path,
                    org,
                    remote,
                    ports,
                    dir,
                    command,
                } => {
                    let name = compose_workspace_name(&name, org.as_deref())?;
                    berth::validate_workspace_name(&name)?;
                    if let Some(host) = &remote {
                        berth::validate_ssh_host(host)?;
                    }
                    commands::new::run(commands::new::NewArgs {
                        name,
                        path,
                        remote,
                        ports,
                        remote_dir: dir,
                        command,
                    })
                    .await
                }
                Commands::Set {
                    name,
                    remote,
                    clear_remote,
                    dir,
                    clear_dir,
                    ports,
                    clear_ports,
                    clear_command,
                    command,
                } => {
                    berth::validate_workspace_name(&name)?;
                    commands::project::set(commands::project::SetArgs {
                        name,
                        remote,
                        clear_remote,
                        dir,
                        clear_dir,
                        ports,
                        clear_ports,
                        command,
                        clear_command,
                    })
                    .await
                }
                Commands::Rm { name } => {
                    berth::validate_workspace_name(&name)?;
                    commands::delete::run(name).await
                }
                Commands::Enter {
                    name,
                    org,
                    remote,
                    ports,
                    dir,
                    plain,
                    auto_deploy,
                    no_deploy,
                    new,
                    no_reconnect,
                    command,
                } => {
                    let name = compose_workspace_name(&name, org.as_deref())?;
                    berth::validate_workspace_name(&name)?;
                    if let Some(host) = &remote {
                        berth::validate_ssh_host(host)?;
                    }
                    let opts = commands::enter::EnterOptions {
                        plain,
                        auto_deploy,
                        no_deploy,
                        force_new: new,
                        no_reconnect,
                        dir,
                        command,
                    };
                    commands::enter::run(name, remote, ports, opts).await
                }
                Commands::Attach {
                    name,
                    new,
                    resume_or_new,
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
                            resume_or_new,
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
                Commands::Stop { name } => {
                    berth::validate_workspace_name(&name)?;
                    commands::stop::run(name).await
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
                Commands::Tunnel { name, ports } => {
                    berth::validate_workspace_name(&name)?;
                    commands::tunnel::run(name, ports).await
                }
                Commands::Org(command) => match command {
                    OrgCommands::List => commands::org::list().await,
                    OrgCommands::Show { name } => commands::org::show(name).await,
                    OrgCommands::Set { name, remote, root } => {
                        if let Some(host) = &remote {
                            berth::validate_ssh_host(host)?;
                        }
                        commands::org::set(name, remote, root).await
                    }
                    OrgCommands::Rm { name } => commands::org::remove(name).await,
                },
                Commands::Hosts(command) => match command {
                    HostsCommands::Update => commands::hosts::update().await,
                    HostsCommands::Clean => commands::hosts::clean().await,
                    HostsCommands::Install => commands::hosts::install().await,
                },
                Commands::Shell(sub) => match sub {
                    ShellSubcommands::Init { shell } => commands::shell::run_init(shell),
                    ShellSubcommands::Completions { shell } => {
                        commands::shell::run_completions(shell)
                    }
                },
                Commands::Logs {
                    lines,
                    follow,
                    sessions,
                } => commands::logs::run(lines, follow, sessions).await,
                Commands::Doctor => commands::doctor::run().await,
                Commands::Daemon {
                    interval_seconds,
                    once,
                } => commands::daemon::run(Some(interval_seconds), once).await,
                Commands::Reap => commands::reap::run().await,
                Commands::Deploy { host, tag, force } => {
                    berth::validate_ssh_host(&host)?;
                    if let Some(t) = &tag {
                        berth::validate_release_tag(t)?;
                    }
                    commands::deploy::run(host, tag, force).await
                }
                Commands::Agent { ports } => commands::agent::run(ports).await,
            }
        } else {
            commands::list::run(false, false).await
        }
    }
}
