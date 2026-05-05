use anyhow::Result;
use berth::config::{Config, Runtime};
use berth::runtime::{self, CommandSpec};
use berth::ssh;
use std::path::Path;

fn remote_projects_path() -> String {
    "$HOME/.local/share/berth/projects".to_string()
}

pub async fn run(
    name: String,
    command: Vec<String>,
    ports: Vec<u16>,
    remote_override: Option<String>,
) -> Result<()> {
    let config = Config::load()?;

    let workspace = config
        .workspaces
        .get(&name)
        .ok_or_else(|| anyhow::anyhow!("Workspace '{}' not found", name))?;

    // Determine remote - use override or workspace config
    let remote = remote_override.or_else(|| workspace.remote.clone());

    // Ports require a remote
    if !ports.is_empty() && remote.is_none() {
        eprintln!("Ports (-p) have no effect for local workspaces. Ignoring.");
    }

    let cmd_str = command.join(" ");
    if cmd_str.is_empty() {
        eprintln!("No command specified");
    }

    match remote {
        Some(host) => {
            // Remote execution
            let tunnel_active = if !ports.is_empty() {
                ssh::start_tunnel(&host, &name, &ports).await?
            } else {
                false
            };

            let remote_path = format!("{}/{}", remote_projects_path(), name);
            let full_cmd = format!(
                "cd {} && nohup {} >/dev/null 2>&1 & disown",
                remote_path, cmd_str
            );

            println!("Running on {}: cd {} && {}", host, remote_path, cmd_str);

            let output = ssh::run_remote_command(&host, &full_cmd).await?;

            if !output.is_empty() {
                println!("{}", output);
            } else {
                println!("Command started successfully.");
            }

            if tunnel_active && !ports.is_empty() {
                println!("Tunnel active: http://localhost:{}", ports[0]);
            }
        }
        None => {
            let local_path = Path::new(&workspace.path);
            let runtime_config = config.merged_runtime_for(workspace, false);
            let mounts = config.merged_mounts(workspace);
            let spec = local_run_spec(&name, local_path, &runtime_config, &mounts, &command)?;
            println!("Running locally in {}: {}", local_path.display(), cmd_str);

            let output = runtime::output_command(&spec)?;

            if !output.status.success() {
                eprintln!(
                    "Command failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                anyhow::bail!("Command failed");
            }

            println!("{}", String::from_utf8_lossy(&output.stdout));
        }
    }

    Ok(())
}

fn local_run_spec(
    name: &str,
    path: &Path,
    runtime_config: &Runtime,
    mounts: &[berth::config::Mount],
    command: &[String],
) -> Result<CommandSpec> {
    match runtime_config {
        Runtime::Bare => Ok(berth::runtime::bare::build_command(
            &berth::runtime::bare::BareRunConfig::new(path, command.iter().cloned()),
        )?),
        Runtime::Podman(podman) => {
            runtime::validate_configured_mounts(mounts)?;
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
                berth::runtime::podman::PodmanRunConfig::new(&podman.image, path, command.to_vec())
                    .with_mounts(runtime_mounts)
                    .with_tty(false)
                    .with_interactive(false);
            config.project = config
                .project
                .with_target(std::path::PathBuf::from(&podman.project_mount));
            let mut spec = berth::runtime::podman::build_command(&config)?;
            spec.program = podman.binary.clone();
            spec.args.splice(
                1..1,
                [
                    "--name".to_string(),
                    format!("berth-{}", name.replace('/', "-")),
                ],
            );
            if let Some(userns) =
                berth::discovery::podman_userns_arg(&podman.binary, podman.userns.as_deref())
            {
                spec.args.splice(1..1, [userns]);
            }
            Ok(spec)
        }
        Runtime::KubernetesPod(kubernetes) => Ok(berth::runtime::kubernetes::build_run_command(
            &berth::runtime::kubernetes::KubernetesRunConfig::new(
                name,
                kubernetes.clone(),
                command.iter().cloned(),
            ),
        )?),
        Runtime::Auto => anyhow::bail!("Auto runtime was not resolved before local run"),
    }
}
