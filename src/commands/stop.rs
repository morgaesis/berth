use anyhow::{bail, Result};
use berth::config::{Config, Runtime};
use berth::runtime;

pub async fn run(name: String) -> Result<()> {
    let config = Config::load()?;

    let workspace = if let Some(workspace) = config.workspaces.get(&name) {
        workspace
    } else {
        bail!("Workspace '{}' not found", name);
    };

    match config.merged_runtime(workspace) {
        Runtime::Bare => {
            println!(
                "Workspace '{}' uses bare runtime. Exit active shells manually.",
                name
            );
        }
        Runtime::Podman(podman) => {
            let container = format!("berth-{}", name.replace('/', "-"));
            let mut spec = berth::runtime::podman::build_stop_command(
                &berth::runtime::podman::PodmanStopConfig::new(&container),
            )?;
            spec.program = podman.binary.clone();
            let status = runtime::run_command(&spec);
            match status {
                Ok(status) if status.success() => println!("Stopped container '{}'.", container),
                Ok(_) => println!(
                    "Container '{}' was not running or could not be stopped.",
                    container
                ),
                Err(error) => println!("Could not run {} stop: {}", podman.binary, error),
            }
        }
        Runtime::KubernetesPod(kubernetes) => {
            let pod_name = berth::runtime::kubernetes::pod_name(&name, &kubernetes);
            let spec = berth::runtime::kubernetes::build_delete_command(
                &berth::runtime::kubernetes::KubernetesDeleteConfig::new(&name, kubernetes.clone()),
            )?;
            let status = runtime::run_command(&spec);
            match status {
                Ok(status) if status.success() => println!("Deleted pod '{}'.", pod_name),
                Ok(_) => println!(
                    "Pod '{}' was not running or could not be deleted.",
                    pod_name
                ),
                Err(error) => {
                    println!("Could not run {} delete pod: {}", kubernetes.kubectl, error)
                }
            }
        }
        Runtime::Auto => println!("Workspace '{}' uses unresolved auto runtime.", name),
    }

    let _ = berth::lifecycle_state::remove(&name, workspace.remote.as_deref());

    Ok(())
}
