# Architecture

The CLI is a thin command dispatcher. Commands load configuration, resolve workspace defaults, then delegate to runtime, SSH, tunnel, and lifecycle modules.

Source-of-truth order:

1. CLI arguments override workspace config for transient fields such as remote host and ports.
2. Workspace config overrides global defaults for runtime, mounts, remote options, and idle policy.
3. `runtime.type: auto` is the built-in default for local workspaces. It discovers local Podman and falls back to bare when Podman is unavailable. Remote auto runtime resolves to bare because Berth does not probe remote hosts.
4. Explicit `runtime.type: bare` is the primary opt-out for containerized local workspaces.

Discovery is local-only. `discovery` probes `podman`, `kubectl`, and `minikube` without reading secret-bearing environment values, contacting remote hosts, or mutating Kubernetes clusters. Minikube enables Kubernetes pod defaults only when a rootless Podman profile or config is detected. `berth doctor` is the user-facing source for discovery decisions.

Runtime modules own command construction. `runtime::bare` builds direct argv commands with project cwd. `runtime::podman` builds rootless Podman commands with project and configured bind mounts; local commands auto-detect whether `--userns=keep-id` works and omit it when the host runtime rejects it. `runtime::kubernetes` builds `kubectl run` and `kubectl delete pod` commands from workspace runtime config. Commands execute `CommandSpec` through the runtime executor. Fake execution is only a fast-path test seam; e2e coverage should use real runtimes and real processes.

Remote behavior is SSH-first and install-free by default. Berth may use tools already available on the remote host, such as `tmux` or `screen`, but must not require package installation. Plain SSH does not provide mosh-style transport resumability and cannot reattach to an arbitrary orphaned interactive process unless that process was launched under a remote multiplexer or supervisor. The source of truth for resumability is therefore: use `tmux` first when present, use `screen` second when present, otherwise run a direct SSH shell with no reattach guarantee.

Daemon-assisted resumability is a future opt-in mode, not the default contract. If implemented, the local CLI should copy or stream a compatible `berth` helper to a user-owned data path on the remote host, start a per-workspace supervisor, and speak a small attach/detach protocol over SSH stdio. That mode can provide reattach semantics without UDP, but it is no longer "use only what is already available there" and must be gated by explicit config.

Remote container entry mirrors local Podman command construction as shell text executed over SSH.

Lifecycle state records active workspace/runtime metadata under the Berth data directory. Idle TTL parsing and state-transition math live in `lifecycle`; persisted environment activity lives in `lifecycle_state`. `berth reap` scans that state and stops expired local runtime environments. Podman reaping uses the configured podman binary and deterministic container names. Kubernetes pod reaping uses configured namespace and pod names, and should grow labels/annotations as runtime-owned metadata before broader cluster discovery is added.

The local daemon is an explicit foreground process. Its first responsibility is periodic idle reaping through the same `reap::run_once` path used by `berth reap`; it should not grow separate shutdown semantics. Future daemon-assisted remote resumability should build on this supervision boundary, but remote executable placement must remain explicit opt-in.

E2E tests must validate external behavior against real tools when the feature depends on those tools. Docker compose fixtures are acceptable for SSH targets and nested runtimes when they keep host setup reproducible, but missing container/SSH capabilities should skip or fail as environment prerequisites rather than being replaced by fake-only assertions.
