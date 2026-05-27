# Architecture

The CLI is a thin command dispatcher. Commands load configuration, resolve
workspace defaults, then delegate to runtime, SSH, tunnel, and lifecycle
modules. Subcommand help is the source of truth for invocation; this
document is the source of truth for *why* things happen.

## Resolution order

When a command needs an effective value for a knob (remote host,
working dir, ports, command), it walks this cascade and stops at the
first hit:

1. **CLI argument** — `--remote`, `--dir`, `--ports`, trailing `-- argv`.
   Transient; nothing is written back to config from a flag.
2. **Workspace** — `workspace.remote`, `workspace.remote_dir`,
   `workspace.ports`, `workspace.command` in `config.yaml`.
3. **Org defaults** — when the workspace name is `<org>/<project>`, look
   up `orgs.<org>` and inherit `remote`, `remote_user`, and
   `<remote_root>/<project>`.
4. **Global defaults** — `defaults.runtime`, `defaults.idle`, etc.
5. **Auto-discovery** — `runtime.type: auto` discovers local Podman and
   falls back to bare. Remote auto resolves to bare unconditionally
   because berth never probes a remote for runtime selection.

`berth show <name>` prints the resolved view; values colored as
`(inherited)` came from layer 3 or 4.

## Local runtimes

### Bare

Direct shell or command in the project directory. No isolation, no
mounts. This is the fallback when nothing else is configured and is the
forced choice for remote auto-runtime.

### Podman

`runtime::podman` builds a rootless `podman run` argv:

- Project directory mounted read/write at `runtime.project_mount`
  (default `/workspace`).
- Each `mounts:` entry mounted readonly by default; `readonly: false`
  flips it. `required: true` makes a missing source a hard error
  instead of a silent skip.
- `--userns=keep-id` is added when local discovery confirms it works;
  otherwise omitted. Override with `runtime.userns` in config.
- Container name is `berth-<sanitized-workspace>` (deterministic — used
  by `berth stop`/`berth reap`).

### Kubernetes pod

`runtime::kubernetes` builds `kubectl run` for `enter`/`run` and
`kubectl delete pod` for `stop`/reap. It only contacts a cluster when
those explicit commands are invoked; discovery itself never reads
kube state.

## Discovery

`discovery` probes `podman`, `kubectl`, and `minikube` without reading
secret-bearing environment values, contacting remote hosts, or mutating
clusters. Minikube enables Kubernetes-pod defaults only when a rootless
Podman profile or config is detected. `berth doctor` is the user-facing
source for discovery decisions — if doctor says a runtime is missing,
auto-selection will not pick it.

## Lifecycle, reap, and the daemon

Lifecycle state lives under the berth data directory in `lifecycle.json`.
`lifecycle_state::touch` writes a record on every `berth enter`; reap
reads the records and stops anything past its idle TTL.

- `berth reap` is a one-shot scan. Podman environments are stopped with
  `podman stop berth-<name>`; Kubernetes pods are deleted with
  `kubectl delete pod` using the configured namespace and pod name.
  Bare and remote workspaces are not reaped locally.
- `berth daemon` runs the same scan on `--interval-seconds N`, in the
  foreground. It does not install systemd units, create timers, or
  modify remote hosts. `--once` makes it a single iteration for use
  under an external supervisor.

## Remote entry

Remote entry is SSH-first with batteries-included deployability. On
`berth enter --remote <host> <name>` the cascade is:

1. **Pre-flight probe** — single SSH round-trip, busybox-compatible:
   detect OS/architecture, check for an installed remote `berth`, and
   read its `--version`.
2. **Deploy decision** — if no compatible remote berth is present and
   the host's OS/arch is in the build matrix, the user is prompted
   (TTY) or auto-deployed (`--auto-deploy` or trusted host). The
   matching musl-static binary is fetched from this project's GitHub
   releases (SHA256-verified), `scp`'d to `~/.local/bin/berth`,
   smoke-tested via `<remote-path> --version`, and the host is
   recorded in `config.trusted_hosts` so future enters silently
   redeploy when the version drifts.
3. **Session start** — remote `berth attach --new --session <id> <name>`
   owns the PTY through a per-session Unix socket at
   `runtime_dir/sessions/<sanitized_workspace>/<session_id>.sock`.
   Every `berth enter` invocation gets an independent supervisor, while
   reconnects within that invocation reuse the same generated session id
   when the SSH transport drops. The attach client also
   recognizes `defaults.detach_key` (default `ctrl-]`) as a local
   detach signal: the client exits, while the supervisor and PTY keep
   running for the next attach.

Flags that change the cascade:

- `--auto-deploy` — skip the consent prompt.
- `--no-deploy` — never deploy; fall through to the legacy mux cascade.
- `--plain` (alias `--no-resume`) — bypass everything; open a plain
  SSH login shell with no resumability promise.

When deploy is declined or impossible (architecture outside the build
matrix), berth falls through to a legacy cascade — `mosh-server` →
`tmux` → `screen` → plain shell — each tmux/screen invocation using a
unique `$$-$RANDOM` session suffix so multi-tab usage doesn't pile
into one shared session. There is no pure-shell PTY-resume fallback;
POSIX-sh cannot fake `openpty`/`forkpty`, and shipping a compiled
helper would duplicate the work `berth deploy` already does.

Remote container entry mirrors local Podman command construction as
shell text executed over SSH.

## Session protocol

Workspace and session components of the socket path pass through
`session::sanitize()`. Externally-supplied session ids (e.g.
`berth attach --session <id>`) additionally pass `validate_session_id`.
Workspace names are validated by `validate_workspace_name` to an
ASCII allowlist: `[A-Za-z0-9._-]+(/[A-Za-z0-9._-]+)?`, no `.`/`..`
segments. All remote-command interpolations in
`ssh::remote_enter_command` are shell-quoted defensively even after
that validation.

Session ids are 12 hex chars from a v4 UUID.

## Shell integration

Generated by `berth shell init [bash|zsh]` and consumed via `eval`.
It installs a single hook function: new-tab auto-entry. There are no
command shorthands.

The auto-entry signal cascade for new tabs:

1. WezTerm/iTerm2 user variable (OSC 1337 `SetUserVar`) — pane-scoped;
   cheap to emit.
2. `BERTH_PROJECT_HINT` env var — explicit override.
3. OSC 7 cwd report pointing at a per-workspace marker directory under
   `$XDG_STATE_HOME/berth/active/`. This is the only mechanism that
   reliably propagates to new tabs across common emulators today.
4. `$XDG_STATE_HOME/berth/last-active` — single-file fallback for
   environments where OSC 7 cwd inheritance doesn't reach new tabs
   (notably Windows Terminal + WSL). Time-gated to 10 minutes to bound
   the abuse window if another same-uid process plants the file.

Parking the new tab's `$PWD` inside the marker dir would break direnv
and `cd ..`, so when the detector resolves the project name via the
marker path it `cd`s to `$HOME` **before** invoking `berth enter`,
making the marker cwd transient from the user's perspective. The
marker directory is created on enter and never deleted by berth —
other shells inherit the path as their cwd, so removing it would
leave them pointing at a deleted inode.

`berth shell completions [shell]` emits a completion script that
augments `clap_complete` output:

- **zsh** — rewrites the workspace-name positional from `_default` to
  `_berth_workspaces`; same for org-name slots to `_berth_orgs`.
- **bash** — wraps the generated `_berth` so positional slots at
  workspace/org positions call back into `berth list` / `berth org list`.

Other shells (fish, elvish, powershell) get clap's stock output.

## Source-of-truth modules

- `cli.rs` — clap surface. Flat top-level workspace verbs, subcommand
  groups for `org`/`hosts`/`shell`.
- `config/mod.rs` — `Config`, `Workspace`, `Org`, and the resolution
  functions (`resolved_remote`, `resolved_remote_dir`).
- `discovery.rs` — local runtime probing.
- `runtime/{bare,podman,kubernetes}.rs` — command construction.
- `ssh.rs` — remote entry, tunnel, attach plumbing.
- `deploy/` — remote binary push + SHA verification.
- `lifecycle*.rs` — TTL math and persisted activity records.
- `session/` — supervisor, socket protocol, attach.
- `terminal.rs` — OSC emission + marker-dir state for the new-tab hook.

## Testing strategy

E2E tests validate external behavior against real tools when the
feature depends on those tools. Fake-exec paths exist as fast-path
seams for command construction only. Real-podman e2e runs by default
under `cargo test`; opt out with `BERTH_E2E_PODMAN_ENABLED=false`.
Real-kubernetes e2e is opt-in via `BERTH_E2E_K8S_ENABLED=1` because
it needs a reachable cluster, not just `kubectl` on PATH.

Docker-compose fixtures are acceptable for SSH and nested-runtime
targets when they keep host setup reproducible; missing tools should
skip or fail as environment prerequisites rather than be replaced by
fake-only assertions.
