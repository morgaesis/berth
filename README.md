# Berth

Berth gives you named workspaces — local or remote — and one command to
land in them: `berth enter <name>`.

- **Session continuity, no failed SSH.** Close the laptop, hit a tunnel,
  the VPN flaps — the session keeps running on the far side and the
  next `berth enter` quietly reattaches. Scrollback, the half-typed
  sudo prompt, your AI assistant mid-thought — all still there.
- **New tab → new session, fully configured.** Workspaces can carry a
  multi-stage entry command — e.g.
  `berth enter ws -- bash -c "sudo -u dev bash -ic 'cd app && claude'"`.
  With the shell hook installed, **Meta+T** (or Ctrl+Shift+T on Linux)
  opens a tab that lands you in an independent session with the same
  shape: same host, same user, same directory, same tool. Two claude
  sessions side by side in the same environment, set up once.
- **Same shape, local or remote.** `berth enter my-local` and
  `berth enter my-remote` look identical at the prompt. The host
  lives on the workspace itself, set once (`berth set --remote …`,
  or inherited from an org). See
  [examples/config.yaml](./examples/config.yaml).

![Berth Demo](./assets/demo.svg)

## Install

See [INSTALL.md](./INSTALL.md). Quick paths:

```bash
cargo install --git https://github.com/morgaesis/berth --locked
# or
tooler run morgaesis/berth
```

Or hand the link to a coding agent:

> Install berth from <https://github.com/morgaesis/berth>. Read and follow
> `INSTALL.md` at the root of that repo. Verify the install by running
> `berth --version` and `berth doctor`; report both outputs back.

## Usage

```bash
berth list                                       # workspaces + last-used time
berth show myproject                             # resolved config for one
berth new myproject ~/projects/myproject         # create
berth enter myproject                            # create-if-needed, then enter
berth enter myproject -- claude --foo            # override the entry command
berth set myproject --dir ~/code/myproject       # edit fields without yaml
berth rm myproject                               # delete the configuration

berth run myproject -- cargo test                # one-shot command in workspace
berth tunnel myproject -p 3000,8080              # forward remote ports
berth stop myproject                             # stop the runtime
berth reap                                       # stop expired environments
berth daemon --interval-seconds 300              # foreground idle reaper
berth doctor                                     # local runtime + hook status
```

Remote workspaces work the same. `berth enter --remote <host> <ws>` probes
the host, offers a one-time binary deploy when there's no compatible berth
on the other side, and from then on every entry resumes the supervised
session (so a flaky SSH link doesn't kill your shell):

```bash
berth enter --remote prod-box myws               # prompts on first deploy
berth enter --remote prod-box myws --auto-deploy # skip the prompt
berth enter --remote prod-box myws --plain       # plain SSH, no resume
berth deploy prod-box                            # explicit one-shot deploy
```

Org-scoped workspaces (`<org>/<project>`) inherit a remote host and a
remote-root directory from `berth org set`, so you write
`acme/postil` once and stop repeating `--remote` and `--dir`.

## Configuration

Config lives at `$BERTH_CONFIG_DIR/config.yaml` (or
`~/.config/berth/config.yaml`). Most fields are edit-via-CLI
(`berth set`, `berth org set`); hand-editing the yaml is also supported.

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

Re-recording the README's demo asset: `bash scripts/record-demo/record.sh`.
Needs `asciinema` and `svg-term-cli` on the host (`npm i -g svg-term-cli`),
plus the host runtimes berth's `doctor` probes for if you want them shown
as ready. The driver script sandboxes `$HOME` and `PATH` under a tmpdir,
then the wrapper sed-scrubs the tmpdir prefix out of the captured cast
before rendering — so the resulting SVG only ever shows `/home/dev/…`.

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
