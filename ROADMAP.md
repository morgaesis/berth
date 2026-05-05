# Roadmap

Implemented foundation:

- Explicit `berth enter NAME`; implicit `berth NAME` is disabled.
- Shell hook output provides `b NAME`, prompt/title updates, and auto-enter behavior.
- Backward-compatible config schema for runtime, mounts, remote options, and idle policy.
- Auto-discovery for local Podman defaults plus `berth doctor` status output.
- Bare and Podman runtime command builders.
- Podman-backed local `enter` and `run` paths with fast fake-exec coverage plus an env-gated real Podman e2e test.
- `berth reap` stops expired local Podman containers and Kubernetes pod workspaces from lifecycle state.
- `berth daemon` runs in the foreground and periodically invokes the idle reaper without installing hidden services.
- Kubernetes pod runtime command construction for `run`, `stop`, and expired-state reaping with local minikube discovery exposed through `berth doctor`.
- SSH remote entry with best-effort persistence through existing remote `tmux` or `screen`.
- Lifecycle state recording for entered workspaces.

Next work:

- Add CLI flags for setting runtime, image, mounts, and idle TTL during `new`.
- Expand real runtime e2e coverage beyond `run`: verify shell entry, stop, and idle reaping behavior against real processes.
- Add optional docker compose e2e fixtures for SSH targets and nested container runtimes. Treat required tools, privileges, and Linux capabilities as fixture prerequisites, not reasons to weaken e2e assertions.
- Expand real runtime e2e coverage for `berth reap` beyond daemon one-shot coverage against live Podman containers.
- Extend Kubernetes pod workspaces with project mount strategy plus labels/annotations for ownership and idle metadata before adding broader cluster discovery.
- Add an opt-in daemon-assisted remote mode on top of the local foreground daemon boundary. The remote path should copy or stream a compatible `berth` helper to a user-owned remote data path, start a per-workspace supervisor, and expose attach/detach over SSH stdio. It should not require UDP ports, but it does require explicit user consent because it places executable code on the remote host.
- Make `stop` exercise real runtime behavior and add e2e coverage for Podman stop.
- Store tunnel process metadata and health checks instead of only workspace-to-port mappings.
- Split e2e tests by behavior area as the suite grows.
