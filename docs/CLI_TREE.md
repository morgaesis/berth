# Berth CLI Help Tree

Generated from `berth --help` and every visible subcommand help after the CLI simplification pass.

```text
Consistent development workspaces, local or remote, bare metal

Usage: berth [OPTIONS] [COMMAND]

Commands:
  config  Manage workspace config
  enter   Enter a workspace (creates if needed)
  attach  Attach to or start a resumable workspace session
  stop    Stop a workspace
  run     Run a command on a workspace
  tunnel  Tunnel remote ports locally
  org     Manage per-org defaults (remote host + remote-root path)
  hosts   Manage /etc/hosts entries for workspaces
  shell   Shell integration: init script (new-tab hook) + tab completions
  logs    Show recent berth logs
  doctor  Show shell-integration + local runtime status
  deploy  Deploy the berth binary to a remote host over SSH
  help    Print this message or the help of the given subcommand(s)

Options:
  -v, --verbose...  Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged
  -q, --quiet       Silence stderr log output entirely (overrides -v and RUST_LOG)
  -h, --help        Print help
  -V, --version     Print version

Tab completions: `berth shell completions <shell>`
New-tab auto-entry: `eval "$(berth shell init)"` in your rc (see `berth doctor`)


$ berth enter --help
Enter a workspace, creating it if absent.

Workspaces can be plain (`proj`) or org-scoped (`org/proj`).
Use --org to compose an org with a bare project name. Org-scoped
workspaces inherit a remote host and a remote-root directory from
`orgs.<org>` in config (see `berth org set`).

Examples:
  berth enter proj --org org
  berth enter org/proj --remote dev-box
  berth enter org/proj --dir '~/code/org/proj'
  berth enter org/proj -- claude --dangerously-skip-permissions

For remote workspaces, berth probes the host and selects the best
session-mux available. If none, you'll be prompted to deploy the
berth binary to the remote (one-time consent, persisted in config).

Resumability flags:
  --plain         skip session-mux entirely; plain SSH login shell
  --auto-deploy   deploy without prompting (overrides per-host trust)
  --no-deploy     never deploy; fall through to legacy multiplexers

New-tab replay: with the shell hook installed (see `berth doctor`),
  new terminal tabs spawned from a berth session will re-run this same
  invocation verbatim — including any trailing `-- <argv>` override. If
  that command prompts interactively (e.g. sudo), the prompt will
  reappear in each new tab. Set BERTH_SKIP_AUTO=1 to opt out for one
  shell.

Usage: berth enter [OPTIONS] <NAME> [COMMAND]...

Arguments:
  <NAME>
          Workspace name (org/project, or bare project paired with --org)

  [COMMAND]...
          Override workspace default command (everything after `--`)

Options:
  -o, --org <ORG>
          Prepend this org to the workspace name (e.g. --org org proj → org/proj)

  -v, --verbose...
          Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged

  -q, --quiet
          Silence stderr log output entirely (overrides -v and RUST_LOG)

  -r, --remote <REMOTE>
          SSH host (overrides workspace/org default)

  -p, --ports <PORTS>
          Forward remote port(s), repeatable or comma-separated

  -d, --dir <DIR>
          Override the remote working directory (e.g. ~/code/proj)

      --plain
          Skip session-mux; just open a plain SSH login shell

      --auto-deploy
          Deploy berth binary to the remote without prompting

      --no-deploy
          Never deploy; use legacy multiplexers or fail

      --no-reconnect
          Disable the auto-reconnect loop on SSH-drop; bail on first connection loss

  -h, --help
          Print help (see a summary with '-h')


$ berth attach --help
Resume a workspace session managed by the local berth supervisor.

By default, attaches to the single existing session for the workspace.
With --new, always starts a fresh independent session (used by the remote
bootstrap of `berth enter` so each terminal tab gets its own PTY).
With --session <id>, targets a specific session by id.

Usage: berth attach [OPTIONS] <NAME> [COMMAND]...

Arguments:
  <NAME>
          Workspace name (org/project format allowed)

  [COMMAND]...
          Override session command (defaults to login shell)

Options:
      --new
          Start a fresh independent session instead of resuming

  -v, --verbose...
          Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged

  -q, --quiet
          Silence stderr log output entirely (overrides -v and RUST_LOG)

      --session <ID>
          Attach to a specific session id (see `berth attach --list`)

      --list
          List active sessions for the workspace and exit

      --all
          With --list, include exited sessions that still have logs

      --long
          With --list, show status, attachment state, and log presence

  -h, --help
          Print help (see a summary with '-h')


$ berth config --help
Manage workspace config

Usage: berth config [OPTIONS] <COMMAND>

Commands:
  list   List configured workspaces
  show   Show one workspace config
  set    Create or update a workspace config
  unset  Unset workspace config fields
  rm     Delete a workspace config
  help   Print this message or the help of the given subcommand(s)

Options:
  -v, --verbose...  Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged
  -q, --quiet       Silence stderr log output entirely (overrides -v and RUST_LOG)
  -h, --help        Print help


$ berth config list --help
List configured workspaces

Usage: berth config list [OPTIONS]

Options:
  -l, --long        Show resolved config blocks
  -v, --verbose...  Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged
      --abs         Render last-used as absolute UTC timestamps
  -q, --quiet       Silence stderr log output entirely (overrides -v and RUST_LOG)
  -h, --help        Print help


$ berth config show --help
Show one workspace config

Usage: berth config show [OPTIONS] <NAME>

Arguments:
  <NAME>  Workspace name

Options:
  -v, --verbose...  Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged
  -q, --quiet       Silence stderr log output entirely (overrides -v and RUST_LOG)
  -h, --help        Print help


$ berth config set --help
Create or update a workspace config

Usage: berth config set [OPTIONS] <NAME> [COMMAND]...

Arguments:
  <NAME>        Workspace name
  [COMMAND]...  Set default command (everything after `--`)

Options:
      --path <PATH>      Local path for the workspace when creating it
  -v, --verbose...       Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged
  -q, --quiet            Silence stderr log output entirely (overrides -v and RUST_LOG)
  -r, --remote <REMOTE>  Set SSH host
  -d, --dir <DIR>        Set remote working directory
  -p, --ports <PORTS>    Forward remote port(s), repeatable or comma-separated
  -h, --help             Print help


$ berth config unset --help
Unset workspace config fields

Usage: berth config unset [OPTIONS] <NAME> [FIELDS]...

Arguments:
  <NAME>       Workspace name
  [FIELDS]...  Field(s) to unset: remote dir ports command [possible values: remote, dir, ports, command]

Options:
  -v, --verbose...  Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged
  -q, --quiet       Silence stderr log output entirely (overrides -v and RUST_LOG)
  -h, --help        Print help


$ berth config rm --help
Delete a workspace config

Usage: berth config rm [OPTIONS] <NAME>

Arguments:
  <NAME>  Workspace name

Options:
  -v, --verbose...  Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged
  -q, --quiet       Silence stderr log output entirely (overrides -v and RUST_LOG)
  -h, --help        Print help


$ berth org --help
Configure defaults for workspace names of the form `<org>/<project>`. A workspace can inherit its remote host and remote working-directory root from its org, so individual workspaces don't have to repeat the prefix.

Examples:
  berth org set org --remote dev-box --user dev --root '~/code/org'
  berth org list
  berth org show org

Usage: berth org [OPTIONS] <COMMAND>

Commands:
  list  List all configured orgs
  show  Show one org's defaults
  set   Set or update an org's defaults
  rm    Remove an org from config (doesn't touch any workspace)
  help  Print this message or the help of the given subcommand(s)

Options:
  -v, --verbose...
          Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged

  -q, --quiet
          Silence stderr log output entirely (overrides -v and RUST_LOG)

  -h, --help
          Print help (see a summary with '-h')


$ berth org set --help
Set or update an org's defaults

Usage: berth org set [OPTIONS] <NAME>

Arguments:
  <NAME>  Org name (e.g. org)

Options:
  -r, --remote <REMOTE>  Default SSH host for workspaces in this org
  -v, --verbose...       Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged
  -q, --quiet            Silence stderr log output entirely (overrides -v and RUST_LOG)
  -R, --root <ROOT>      Default remote-root directory (final workspace dir = <root>/<project>)
      --user <USER>      Default SSH user for this org
  -h, --help             Print help


$ berth org show --help
Show one org's defaults

Usage: berth org show [OPTIONS] <NAME>

Arguments:
  <NAME>  Org name

Options:
  -v, --verbose...  Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged
  -q, --quiet       Silence stderr log output entirely (overrides -v and RUST_LOG)
  -h, --help        Print help


$ berth org list --help
List all configured orgs

Usage: berth org list [OPTIONS]

Options:
  -v, --verbose...  Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged
  -q, --quiet       Silence stderr log output entirely (overrides -v and RUST_LOG)
  -h, --help        Print help


$ berth org rm --help
Remove an org from config (doesn't touch any workspace)

Usage: berth org rm [OPTIONS] <NAME>

Arguments:
  <NAME>  Org name

Options:
  -v, --verbose...  Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged
  -q, --quiet       Silence stderr log output entirely (overrides -v and RUST_LOG)
  -h, --help        Print help


$ berth shell --help
Generate the new-tab auto-entry hook and tab-completion scripts.

Examples:
  eval "$(berth shell init)"             # source the new-tab hook in your rc
  eval "$(berth shell completions)"      # source completions in your rc
  berth shell init bash > ~/.config/berth/init.sh
  berth shell completions zsh > ~/.zsh/completions/_berth

Usage: berth shell [OPTIONS] <COMMAND>

Commands:
  init         Print the new-tab auto-entry hook script
  completions  Print the completion script for the given shell
  help         Print this message or the help of the given subcommand(s)

Options:
  -v, --verbose...
          Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged

  -q, --quiet
          Silence stderr log output entirely (overrides -v and RUST_LOG)

  -h, --help
          Print help (see a summary with '-h')


$ berth shell init --help
Print a shell init script. Source via `eval "$(berth shell init)"` in your bashrc/zshrc. The script hooks new shells so that, when opened from inside a berth workspace, they auto-re-enter the same workspace with the same command override.

Usage: berth shell init [OPTIONS] [SHELL]

Arguments:
  [SHELL]
          Target shell (auto-detected from $SHELL when omitted)
          
          [possible values: bash, zsh]

Options:
  -v, --verbose...
          Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged

  -q, --quiet
          Silence stderr log output entirely (overrides -v and RUST_LOG)

  -h, --help
          Print help (see a summary with '-h')


$ berth shell completions --help
Emit completion script for the given shell. Auto-detects from $SHELL when omitted.

Install (zsh):  berth shell completions zsh  > ~/.zsh/completions/_berth
Install (bash): berth shell completions bash > ~/.local/share/bash-completion/completions/berth

Usage: berth shell completions [OPTIONS] [SHELL]

Arguments:
  [SHELL]
          Target shell (auto-detected from $SHELL when omitted)
          
          [possible values: bash, elvish, fish, powershell, zsh]

Options:
  -v, --verbose...
          Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged

  -q, --quiet
          Silence stderr log output entirely (overrides -v and RUST_LOG)

  -h, --help
          Print help (see a summary with '-h')


$ berth hosts --help
Manage /etc/hosts entries for workspaces

Usage: berth hosts [OPTIONS] <COMMAND>

Commands:
  update   Update hosts file with all workspace names
  clean    Remove all berth entries from hosts file
  install  Add wildcard *.berth entry to hosts file (requires sudo)
  help     Print this message or the help of the given subcommand(s)

Options:
  -v, --verbose...  Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged
  -q, --quiet       Silence stderr log output entirely (overrides -v and RUST_LOG)
  -h, --help        Print help


$ berth hosts update --help
Update hosts file with all workspace names

Usage: berth hosts update [OPTIONS]

Options:
  -v, --verbose...  Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged
  -q, --quiet       Silence stderr log output entirely (overrides -v and RUST_LOG)
  -h, --help        Print help


$ berth hosts clean --help
Remove all berth entries from hosts file

Usage: berth hosts clean [OPTIONS]

Options:
  -v, --verbose...  Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged
  -q, --quiet       Silence stderr log output entirely (overrides -v and RUST_LOG)
  -h, --help        Print help


$ berth hosts install --help
Add wildcard *.berth entry to hosts file (requires sudo)

Usage: berth hosts install [OPTIONS]

Options:
  -v, --verbose...  Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged
  -q, --quiet       Silence stderr log output entirely (overrides -v and RUST_LOG)
  -h, --help        Print help


$ berth stop --help
Stop a workspace

Usage: berth stop [OPTIONS] <NAME>

Arguments:
  <NAME>  Workspace name (org/project format allowed)

Options:
  -v, --verbose...  Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged
  -q, --quiet       Silence stderr log output entirely (overrides -v and RUST_LOG)
  -h, --help        Print help


$ berth run --help
Run a command on a workspace

Usage: berth run [OPTIONS] <NAME> [COMMAND]...

Arguments:
  <NAME>        Workspace name (org/project format allowed)
  [COMMAND]...  

Options:
  -r, --remote <REMOTE>  Override remote SSH host
  -v, --verbose...       Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged
  -p, --ports <PORTS>    Start tunnel for these ports (requires remote)
  -q, --quiet            Silence stderr log output entirely (overrides -v and RUST_LOG)
  -h, --help             Print help


$ berth tunnel --help
Tunnel remote ports locally

Usage: berth tunnel [OPTIONS] <NAME>

Arguments:
  <NAME>  Workspace name (org/project format allowed)

Options:
  -p, --ports <PORTS>  
  -v, --verbose...     Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged
  -q, --quiet          Silence stderr log output entirely (overrides -v and RUST_LOG)
  -h, --help           Print help


$ berth logs --help
Show recent berth logs.

Includes global logs and, with --sessions, session supervisor logs. Use --follow to stream new entries.

Examples:
  berth logs --level warn
  berth logs --follow --level warn

Usage: berth logs [OPTIONS]

Options:
  -n, --lines <LINES>
          Tail length (default 200)

  -v, --verbose...
          Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged

      --follow
          Follow new log lines

  -q, --quiet
          Silence stderr log output entirely (overrides -v and RUST_LOG)

      --level <LEVEL>
          Only show log lines at this level or higher
          
          [possible values: trace, debug, info, warn, error]

      --sessions
          Always include per-session supervisor logs even with -n

  -h, --help
          Print help (see a summary with '-h')


$ berth doctor --help
Show shell-integration + local runtime status

Usage: berth doctor [OPTIONS]

Options:
  -v, --verbose...  Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged
  -q, --quiet       Silence stderr log output entirely (overrides -v and RUST_LOG)
  -h, --help        Print help


$ berth deploy --help
Probe the remote host for OS+architecture, fetch the matching
pre-built berth binary from this project's GitHub releases (verifying
SHA256), and scp it to ~/.local/bin/berth on the remote.

Subsequent `berth enter --remote <host>` invocations will then run
`berth attach --new --session <id> <ws>` on the far side, so each
enter invocation gets an independent session while transport reconnects
return to that same session.

Adds the host to `trusted_hosts` in the config on success so future
enters auto-deploy without prompting when the remote binary is stale
or missing.

Usage: berth deploy [OPTIONS] <HOST>

Arguments:
  <HOST>
          SSH host (anything `ssh <host>` would accept)

Options:
      --tag <TAG>
          GitHub release tag to fetch (defaults to v<this-binary-version>)

  -v, --verbose...
          Increase log verbosity (-v info, -vv debug, -vvv trace). Overrides RUST_LOG when set; otherwise RUST_LOG is honored unchanged

      --force
          Redeploy even if the remote binary matches

  -q, --quiet
          Silence stderr log output entirely (overrides -v and RUST_LOG)

  -h, --help
          Print help (see a summary with '-h')

```
