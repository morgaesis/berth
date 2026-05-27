# Berth

Berth gives you named workspaces — local or remote — and one command to
land in them: `berth enter <name>`.

- **Session continuity, no failed SSH.** Close the laptop, hit a tunnel,
  the VPN flaps — the current `berth enter` invocation reconnects to
  the same far-side session until you detach or exit it. Later,
  `berth attach` is the explicit way back to an existing session.
- **New tab → new session, fully configured.** Workspaces can carry a
  multi-stage entry command — e.g.
  `berth enter my-project -- bash -c "sudo -u dev bash -ic 'cd app && claude'"`.
  With the shell hook installed, **Meta+T** (or Ctrl+Shift+T on Linux)
  opens a tab that lands you in an independent session with the same
  shape: same host, same user, same directory, same tool. Two claude
  sessions side by side in the same environment, set up once.
- **Same shape, local or remote.** `berth enter my-local` and
  `berth enter my-remote` look identical at the prompt. The host
  lives on the workspace itself, set once (`berth config set --remote …`,
  or inherited from an org). See
  [examples/config.yaml](./examples/config.yaml).

![Berth Demo](./assets/demo.svg)

## Install

See [INSTALL.md](./INSTALL.md). Quick paths:

```bash
cargo install --git https://github.com/morgaesis/berth --locked
```

```bash
tooler run morgaesis/berth
```

Or hand this prompt to a coding agent:

```text
Install berth from https://github.com/morgaesis/berth. Read and follow
INSTALL.md at the root of that repo. Verify the install by running
`berth --version` and `berth doctor`; report both outputs back.
```

## Usage

```bash
berth config list                             # workspaces + last-used time
berth config show my-project                  # resolved config for one
berth config set my-project --path ~/code/my-project  # create/update
berth enter my-project                        # create-if-needed, then enter
berth enter my-project -- claude --foo        # override the entry command
berth config set my-project --dir ~/code/my-project   # edit fields without yaml
berth config rm my-project                    # delete the configuration

berth run my-project -- cargo test            # one-shot command in workspace
berth tunnel my-project -p 3000,8080          # forward remote ports
berth stop my-project                         # stop the runtime
berth reap                                    # stop expired environments
berth daemon --interval-seconds 300           # foreground idle reaper
berth doctor                                  # local runtime + hook status
```

Remote workspaces work the same. `berth enter --remote <host> <name>` probes
the host, offers a one-time binary deploy when there's no compatible berth
on the other side. Each `berth enter` starts a fresh supervised session,
then keeps reconnecting to that same session if the SSH link drops:

```bash
berth enter --remote prod-box my-project                # prompts on first deploy
berth enter --remote prod-box my-project --auto-deploy  # skip the prompt
berth enter --remote prod-box my-project --plain        # plain SSH, no resume
berth deploy prod-box                                   # explicit one-shot deploy
```

Press `Ctrl-]` while attached to detach the client without stopping the
session. Change `defaults.detach_key` in config to another key such as
`ctrl-a` or `esc`, or set it to `null` to disable.

Org-scoped workspaces (`<org>/<project>`) inherit a remote host, remote
user, and remote-root directory from `berth org set`, so you write
`org/proj` once and stop repeating `--remote`, `--user`, and `--dir`.

## Configuration

Config lives at `$BERTH_CONFIG_DIR/config.yaml` (or
`~/.config/berth/config.yaml`). Most fields are edit-via-CLI
(`berth config set`, `berth org set`); hand-editing the yaml is also supported.

[`examples/config.default.yaml`](./examples/config.default.yaml) is the
fully-documented reference — every field, every default, with the
matching CLI flag where one exists. [`examples/config.yaml`](./examples/config.yaml)
is a worked example showing bare, podman, kubernetes-pod, remote, and
org-scoped workspaces.

Internals — how runtimes, deploy, the new-tab hook, and the SSH session
protocol work — live in [ARCHITECTURE.md](./ARCHITECTURE.md). What's
next is in [ROADMAP.md](./ROADMAP.md).

## Development

```bash
cargo build --quiet --release
cargo test --quiet
```

The default `cargo test` runs the full e2e suite — including real
podman and real kubernetes against live runtimes. Per-machine opt-outs
live in a gitignored `.env` at the repo root (see `.env.example`):

```bash
cp .env.example .env
# uncomment BERTH_E2E_PODMAN_ENABLED=false on hosts without podman
# uncomment BERTH_E2E_K8S_ENABLED=false on hosts without a reachable cluster
```

`assets/demo.svg` is hand-edited — open it in any text editor to
tweak. The previous asciinema + svg-term pipeline was deleted; manual
authoring produces a smaller, faster-loading asset whose timing can
be tuned without re-running anything.

## Acknowledgements

Berth stands on a long line of prior art for keeping development
sessions alive across the network and across machines.

- [**mosh**](https://mosh.org/) — the original answer to "my shell
  shouldn't die when my Wi-Fi does." Berth's session-continuity UX
  is a direct descendant.
- [**Eternal Terminal**](https://eternalterminal.dev/) — SSH
  replacement with seamless reconnect; same philosophy applied to
  full TCP transport.
- [**tmux**](https://github.com/tmux/tmux) and
  [**GNU Screen**](https://www.gnu.org/software/screen/) — terminal
  multiplexers. Berth uses them as the fallback session host when a
  remote can't run berth itself.
- [**devcontainers**](https://containers.dev/) — the workspace-as-spec
  pattern that prefigures berth's "named recipe → reproducible
  environment" model.
