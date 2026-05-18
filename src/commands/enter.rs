use anyhow::Result;
use berth::config::{Config, Runtime, Workspace};
use berth::deploy::{self, ConsentMode, DeployDecision};
use berth::runtime::{self, CommandSpec};
use berth::ssh;
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::os::fd::AsFd;
use std::path::Path;

/// User-controllable knobs for `berth enter`.
#[derive(Debug, Clone, Default)]
pub struct EnterOptions {
    /// `--plain` / `--no-resume`: skip all session-mux machinery.
    pub plain: bool,
    /// `--auto-deploy`: push the berth binary without prompting.
    pub auto_deploy: bool,
    /// `--no-deploy`: never push; fall through to legacy mux or fail.
    pub no_deploy: bool,
    /// `--dir`: override the remote working directory for this run.
    pub dir: Option<String>,
    /// Trailing `-- <argv>`: override the workspace default command.
    pub command: Vec<String>,
}

fn default_projects_path() -> std::path::PathBuf {
    if let Ok(dir) = env::var("BERTH_DATA_DIR") {
        return std::path::PathBuf::from(dir).join("projects");
    }
    if let Ok(dir) = env::var("XDG_DATA_HOME") {
        return std::path::PathBuf::from(dir).join("berth").join("projects");
    }

    dirs::data_local_dir()
        .map(|p| p.join("berth").join("projects"))
        .unwrap_or_else(|| std::path::PathBuf::from("~/.local/share/berth/projects"))
}

pub async fn run(
    name: String,
    remote_override: Option<String>,
    ports_override: Vec<u16>,
    opts: EnterOptions,
) -> Result<()> {
    let mut config = Config::load()?;

    let workspace = if let Some(ws) = config.workspaces.get(&name) {
        ws.clone()
    } else {
        let default_path = default_projects_path().join(&name);

        let path_str = default_path.to_string_lossy().to_string();

        if !default_path.exists() {
            fs::create_dir_all(&default_path)?;
            println!("Created directory: {}", path_str);
        }

        let mut workspace = Workspace::new(path_str.clone());
        workspace.remote = remote_override.clone();
        workspace.ports = if ports_override.is_empty() {
            None
        } else {
            Some(ports_override.clone())
        };

        config.workspaces.insert(name.clone(), workspace.clone());
        config.save()?;
        println!("Created workspace '{}' at {}", name, path_str);

        workspace
    };

    let path = Path::new(&workspace.path);
    if !path.exists() {
        fs::create_dir_all(path)?;
    }

    // Resolve effective host: CLI override > workspace.remote >
    // orgs[<org>].remote. Allocate a string only when we fall back to
    // the org-default path so the common case stays cheap.
    let org_host: Option<String> = config.resolved_remote(&name, &workspace);
    let remote = remote_override
        .as_ref()
        .or(workspace.remote.as_ref())
        .or(org_host.as_ref());
    let ports = if !ports_override.is_empty() {
        Some(ports_override.as_slice())
    } else {
        workspace.ports.as_deref()
    };

    let runtime_config = config.merged_runtime_for(&workspace, remote.is_some());
    let mounts = config.merged_mounts(&workspace);
    let idle = config.merged_idle(&workspace);

    if let Some(host) = remote {
        let host = host.clone();
        // Resolve effective remote dir + command (CLI > workspace > org).
        let remote_dir = opts
            .dir
            .clone()
            .or_else(|| config.resolved_remote_dir(&name, &workspace));
        let command: Option<Vec<String>> = if !opts.command.is_empty() {
            Some(opts.command.clone())
        } else {
            workspace.command.clone()
        };
        ensure_remote_ready(&mut config, &host, &opts).await?;
        enter_remote(
            name,
            &host,
            path,
            ports,
            &runtime_config,
            &mounts,
            &opts,
            remote_dir.as_deref(),
            command.as_deref(),
        )
        .await
    } else {
        let _ = berth::lifecycle_state::touch(
            &name,
            None,
            runtime_name(&runtime_config),
            idle.shutdown_after_seconds,
        );
        let result = enter_local(&name, path, &runtime_config, &mounts);
        let _ = berth::lifecycle_state::remove(&name, None);
        result
    }
}

fn enter_local(
    name: &str,
    path: &Path,
    runtime_config: &Runtime,
    mounts: &[berth::config::Mount],
) -> Result<()> {
    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());

    berth::terminal::emit_enter_signals(name);

    match runtime_config {
        Runtime::Bare => {
            let mut child = std::process::Command::new(&shell)
                .current_dir(path)
                .env("BERTH_WORKSPACE", name)
                .env("BERTH_PATH", path.to_string_lossy().as_ref())
                .spawn()?;

            child.wait()?;
        }
        Runtime::Podman(podman) => {
            runtime::validate_configured_mounts(mounts)?;
            let spec = podman_enter_spec(name, path, &shell, podman, mounts)?;
            let status = runtime::run_command(&spec)?;
            if !status.success() {
                anyhow::bail!("Podman environment exited with error");
            }
        }
        Runtime::KubernetesPod(kubernetes) => {
            let spec = kubernetes_enter_spec(name, &shell, kubernetes)?;
            let status = runtime::run_command(&spec)?;
            if !status.success() {
                anyhow::bail!("Kubernetes pod environment exited with error");
            }
        }
        Runtime::Auto => anyhow::bail!("Auto runtime was not resolved before local entry"),
    }
    berth::terminal::emit_exit_signals(name);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn enter_remote(
    name: String,
    host: &str,
    _path: &Path,
    ports: Option<&[u16]>,
    runtime_config: &Runtime,
    mounts: &[berth::config::Mount],
    opts: &EnterOptions,
    remote_dir: Option<&str>,
    command: Option<&[String]>,
) -> Result<()> {
    if let Some(ports) = ports {
        let _tunnel = ssh::start_tunnel(host, &name, ports).await?;
    }

    berth::terminal::emit_enter_signals(&name);

    // `--plain` skips the resumability cascade and opens a plain SSH
    // login shell. The remote-dir override is still honored: the plain
    // path uses `ssh_interactive` which itself `cd`s into the auto path,
    // so for now `--dir` + `--plain` is a no-op-on-dir. (Plumbing the
    // override through `ssh_interactive` is a follow-up.)
    tracing::info!(
        plain = opts.plain,
        has_dir = remote_dir.is_some(),
        has_cmd = command.is_some(),
        "starting remote ssh session"
    );
    let result = if opts.plain {
        ssh::ssh_interactive(host, &name, true).await
    } else {
        let overrides = ssh::RemoteEnterOverrides {
            remote_dir,
            command,
        };
        ssh::ssh_interactive_runtime_with(host, &name, runtime_config, mounts, overrides).await
    };
    tracing::info!(ok = result.is_ok(), "remote ssh session returned");

    berth::terminal::emit_exit_signals(&name);
    tracing::info!("emitted exit signals");

    // Exit-banner: best-effort restate of which version was running
    // remote during the session. Useful in case the user redeploys the
    // remote out-of-band between sessions.
    if !opts.plain {
        let local_version = env!("CARGO_PKG_VERSION");
        // Cheap re-read from trusted_hosts (no network round-trip).
        if let Ok(cfg) = berth::config::Config::load() {
            if let Some(t) = cfg.trusted_hosts.get(host) {
                eprintln!(
                    "berth: session ended on {host}  (local v{local_version} / remote v{})",
                    t.version
                );
            }
        }
    }

    result?;
    Ok(())
}

fn podman_enter_spec(
    name: &str,
    path: &Path,
    shell: &str,
    podman: &berth::config::PodmanRuntime,
    mounts: &[berth::config::Mount],
) -> Result<CommandSpec> {
    let runtime_mounts = mounts
        .iter()
        .map(|mount| {
            if mount.readonly {
                berth::runtime::ConfiguredMount::new(&mount.source, &mount.target)
            } else {
                berth::runtime::ConfiguredMount::read_write(&mount.source, &mount.target)
            }
        })
        .collect::<Vec<_>>();

    let mut config =
        berth::runtime::podman::PodmanRunConfig::new(&podman.image, path, [shell.to_string()])
            .with_mounts(runtime_mounts);
    config.project = config
        .project
        .with_target(std::path::PathBuf::from(&podman.project_mount));
    let mut spec = berth::runtime::podman::build_command(&config)?;
    spec.program = podman.binary.clone();
    let name_arg = format!("berth-{}", name.replace('/', "-"));
    spec.args.splice(1..1, ["--name".to_string(), name_arg]);
    if let Some(userns) =
        berth::discovery::podman_userns_arg(&podman.binary, podman.userns.as_deref())
    {
        spec.args.splice(1..1, [userns]);
    }
    Ok(spec)
}

fn runtime_name(runtime_config: &Runtime) -> &'static str {
    match runtime_config {
        Runtime::Bare => "bare",
        Runtime::Podman(_) => "podman",
        Runtime::KubernetesPod(_) => "kubernetes-pod",
        Runtime::Auto => "auto",
    }
}

fn kubernetes_enter_spec(
    name: &str,
    shell: &str,
    kubernetes: &berth::config::KubernetesPodRuntime,
) -> Result<CommandSpec> {
    Ok(berth::runtime::kubernetes::build_run_command(
        &berth::runtime::kubernetes::KubernetesRunConfig::new(name, kubernetes.clone(), [shell]),
    )?)
}

/// Implement the resumability cascade for remote enter.
///
///   --plain                  → no-op (caller will run plain ssh)
///   --no-deploy              → no-op; SSH cascade will pick mosh/tmux/screen
///                              or plain shell
///   trusted_hosts contains host → silent redeploy if remote is missing/stale
///   --auto-deploy            → deploy without prompt
///   default                  → probe; if remote needs work, prompt the user
///                              (TTY only); on accept, deploy and trust
async fn ensure_remote_ready(config: &mut Config, host: &str, opts: &EnterOptions) -> Result<()> {
    if opts.plain {
        eprintln!("berth: --plain set; opening a plain SSH shell with no resumable session");
        return Ok(());
    }
    if opts.no_deploy {
        return Ok(());
    }

    // Best-effort nag if the local binary is behind the latest GitHub
    // release; never blocks real work.
    deploy::freshness::warn_if_stale().await;

    let local_version = env!("CARGO_PKG_VERSION").to_string();
    let env = match deploy::probe(host).await {
        Ok(env) => env,
        Err(err) => {
            eprintln!(
                "berth: probe of {host} failed ({err:#}); falling through to the SSH cascade"
            );
            return Ok(());
        }
    };

    let decision = deploy::decide(&env, &local_version);
    let already_trusted = config.trusted_hosts.contains_key(host);

    // Always surface the version diff. The deploy header below only
    // fires when we're actually deploying; for an UpToDate remote, the
    // user wouldn't otherwise know what's running there.
    let remote_ver_str = env
        .berth_version
        .as_deref()
        .map(|v| format!("berth {v}"))
        .unwrap_or_else(|| "no remote berth".to_string());
    eprintln!("berth: local v{local_version}  |  {host}: {remote_ver_str}");

    let consent = match (opts.auto_deploy, already_trusted) {
        (true, _) => ConsentMode::AutoApproved,
        (_, true) if config.auto_update_remote => ConsentMode::AutoApproved,
        (_, true) => {
            // Trusted but auto-update disabled. Print a clear hint and
            // treat this run as no-deploy so the legacy mux cascade
            // takes over with whatever's on the remote.
            if matches!(decision, DeployDecision::Deploy { .. }) {
                eprintln!(
                    "berth: auto_update_remote is false; remote stays at {remote_ver_str}. \
                     Run `berth deploy --force {host}` to refresh."
                );
            }
            return Ok(());
        }
        _ => ConsentMode::Ask,
    };

    match decision {
        DeployDecision::UpToDate => Ok(()),
        DeployDecision::UnsupportedArch { os, arch } => {
            anyhow::bail!(
                "berth has no pre-built binary for {os}/{arch} on {host}. \
                 Install tmux/screen on the remote, or rerun with \
                 `berth enter --plain --remote {host} <ws>` to skip session-mux."
            );
        }
        DeployDecision::Deploy { target, reason } => {
            if consent == ConsentMode::Ask
                && !confirm_deploy(host, target, &env, &local_version, &reason)?
            {
                eprintln!(
                    "berth: deploy declined; falling through to the SSH cascade. \
                     Use `--plain` to skip session-mux entirely, or \
                     `berth deploy {host}` later to opt in."
                );
                return Ok(());
            }
            let tag = format!("v{local_version}");
            let info = deploy::ensure_deployed(host, &tag, target)
                .await
                .with_context_hard_fail(host)?;
            deploy::record_trust(config, host, &info)?;
            eprintln!(
                "berth: deployed v{} to {}:{}  (host added to trusted_hosts)",
                info.version,
                host,
                info.remote_path.display()
            );
            Ok(())
        }
    }
}

fn confirm_deploy(
    host: &str,
    target: &'static str,
    env: &berth::deploy::RemoteEnv,
    local_version: &str,
    reason: &str,
) -> Result<bool> {
    if !io::stdin().is_terminal() {
        // Non-interactive: don't prompt; behave like --no-deploy.
        eprintln!("berth: {host} {reason}; running non-interactively, skipping deploy");
        return Ok(false);
    }
    // Make the arch decision auditable BEFORE the prompt so the user can
    // sanity-check that we're not about to push an x86 binary at an ARM
    // box (or vice versa).
    eprintln!("berth: deploy plan for {host}");
    eprintln!(
        "  local:  {} / {}  (v{local_version})",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    eprintln!(
        "  remote: {} / {}  ({})",
        env.os,
        env.arch,
        env.berth_version
            .as_deref()
            .map(|v| format!("berth v{v}"))
            .unwrap_or_else(|| "no existing berth".to_string())
    );
    eprintln!("  target: {target}");
    eprint!("berth: deploy? [Y/n]: ");
    io::stderr().flush().ok();
    let answer = read_yes_no_default_yes()?;
    eprintln!("{}", if answer { "y" } else { "n" });
    Ok(answer)
}

/// Single-keystroke Y/n prompt with Y as the default. Returns true on
/// Y/y/Enter, false otherwise. Restores the original termios state on
/// every exit path including panics, via a Drop guard.
fn read_yes_no_default_yes() -> Result<bool> {
    use nix::sys::termios::{tcgetattr, tcsetattr, LocalFlags, SetArg, Termios};

    struct RawModeGuard {
        original: Termios,
    }
    impl Drop for RawModeGuard {
        fn drop(&mut self) {
            let stdin = io::stdin();
            let _ = tcsetattr(stdin.as_fd(), SetArg::TCSANOW, &self.original);
        }
    }

    let stdin = io::stdin();
    let original = tcgetattr(stdin.as_fd())?;
    let mut raw = original.clone();
    raw.local_flags
        .remove(LocalFlags::ICANON | LocalFlags::ECHO);
    tcsetattr(stdin.as_fd(), SetArg::TCSANOW, &raw)?;
    let _guard = RawModeGuard { original };

    let mut byte = [0u8; 1];
    let n = stdin.lock().read(&mut byte)?;
    if n == 0 {
        return Ok(true); // EOF — fall to the default
    }
    Ok(matches!(byte[0], b'y' | b'Y' | b'\r' | b'\n'))
}

/// Extension trait that converts a deploy failure into a clear hard-fail
/// pointing the user at the `--plain` escape hatch.
trait ContextHardFail<T> {
    fn with_context_hard_fail(self, host: &str) -> Result<T>;
}

impl<T> ContextHardFail<T> for Result<T> {
    fn with_context_hard_fail(self, host: &str) -> Result<T> {
        self.map_err(|e| {
            anyhow::anyhow!(
                "deploy to {host} failed: {e:#}\n\
                 Workarounds:\n  \
                 • `berth enter --plain --remote {host} <ws>` opens a plain SSH session (no resume)\n  \
                 • install tmux or mosh on {host} and rerun without --no-deploy\n  \
                 • run `berth deploy {host}` interactively to inspect the failure"
            )
        })
    }
}
