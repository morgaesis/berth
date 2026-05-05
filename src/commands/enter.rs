use anyhow::Result;
use berth::config::{Config, Runtime, Workspace};
use berth::runtime::{self, CommandSpec};
use berth::ssh;
use std::env;
use std::fs;
use std::path::Path;

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
        enter_remote(name, host, path, ports, &runtime_config, &mounts).await
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

    println!("\x1b]2;berth: {}\x07", name);

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
    Ok(())
}

async fn enter_remote(
    name: String,
    host: &str,
    _path: &Path,
    ports: Option<&[u16]>,
    runtime_config: &Runtime,
    mounts: &[berth::config::Mount],
) -> Result<()> {
    if let Some(ports) = ports {
        let _tunnel = ssh::start_tunnel(host, &name, ports).await?;
    }

    println!("\x1b]2;berth: {} [{}]\x07", name, host);

    ssh::ssh_interactive_runtime(host, &name, runtime_config, mounts).await?;

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
