use anyhow::Result;
use berth::config::{Config, Runtime, Workspace};
use berth::deploy::{self, ConsentMode, DeployDecision};
use berth::runtime::{self, CommandSpec};
use berth::ssh;
use std::env;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;

/// User-controllable knobs for the resumability cascade on remote enter.
#[derive(Debug, Clone, Copy, Default)]
pub struct EnterOptions {
    /// `--plain` / `--no-resume`: skip all session-mux machinery.
    pub plain: bool,
    /// `--auto-deploy`: push the berth binary without prompting.
    pub auto_deploy: bool,
    /// `--no-deploy`: never push; fall through to legacy mux or fail.
    pub no_deploy: bool,
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

    let remote = remote_override.as_ref().or(workspace.remote.as_ref());
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
        ensure_remote_ready(&mut config, &host, &opts).await?;
        enter_remote(name, &host, path, ports, &runtime_config, &mounts, &opts).await
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

async fn enter_remote(
    name: String,
    host: &str,
    _path: &Path,
    ports: Option<&[u16]>,
    runtime_config: &Runtime,
    mounts: &[berth::config::Mount],
    opts: &EnterOptions,
) -> Result<()> {
    if let Some(ports) = ports {
        let _tunnel = ssh::start_tunnel(host, &name, ports).await?;
    }

    berth::terminal::emit_enter_signals(&name);

    // `--plain` skips the resumability cascade and opens a plain SSH
    // login shell that just `cd`s into the workspace dir. No tmux,
    // screen, mosh, or berth-attach.
    let result = if opts.plain {
        ssh::ssh_interactive(host, &name, true).await
    } else {
        ssh::ssh_interactive_runtime(host, &name, runtime_config, mounts).await
    };

    berth::terminal::emit_exit_signals(&name);

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

    let consent = match (opts.auto_deploy, already_trusted) {
        (true, _) => ConsentMode::AutoApproved,
        (_, true) => ConsentMode::AutoApproved,
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
            if consent == ConsentMode::Ask && !confirm_deploy(host, target, &reason)? {
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
                "berth: deployed {} to {} ({})",
                info.version,
                host,
                info.remote_path.display()
            );
            Ok(())
        }
    }
}

fn confirm_deploy(host: &str, target: &'static str, reason: &str) -> Result<bool> {
    if !io::stdin().is_terminal() {
        // Non-interactive: don't prompt; behave like --no-deploy.
        eprintln!("berth: {host} {reason}; running non-interactively, skipping deploy");
        return Ok(false);
    }
    eprint!("berth: deploy berth-{target} to {host}? [y/N] (will be added to trusted_hosts): ");
    io::stderr().flush().ok();
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
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
