use anyhow::Result;
use berth::config::{Config, Runtime};
use berth::lifecycle_state::{self, Environment};
use berth::runtime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub stopped: usize,
    pub skipped: usize,
}

pub async fn run() -> Result<()> {
    let summary = run_once().await?;
    print_summary(summary);
    Ok(())
}

pub async fn run_once() -> Result<Summary> {
    let config = Config::load()?;
    let mut state = lifecycle_state::load();
    let now = lifecycle_state::now();
    let mut stopped = 0usize;
    let mut skipped = 0usize;

    let expired = state
        .environments
        .iter()
        .filter_map(|(key, environment)| {
            should_reap(environment, now).then(|| (key.clone(), environment.clone()))
        })
        .collect::<Vec<_>>();

    for (key, environment) in expired {
        let Some(workspace) = config.workspaces.get(&environment.workspace) else {
            skipped += 1;
            eprintln!(
                "Skipping '{}': workspace '{}' is no longer configured.",
                key, environment.workspace
            );
            continue;
        };

        match config.merged_runtime(workspace) {
            Runtime::Bare => {
                skipped += 1;
                eprintln!("Skipping '{}': bare workspaces are not reaped.", key);
            }
            Runtime::Podman(podman) => {
                let container = podman_container_name(&environment.workspace);
                let mut spec = berth::runtime::podman::build_stop_command(
                    &berth::runtime::podman::PodmanStopConfig::new(&container),
                )?;
                spec.program = podman.binary.clone();

                let status = runtime::run_command(&spec)?;
                if status.success() {
                    state.environments.remove(&key);
                    stopped += 1;
                    println!("Stopped expired container '{}'.", container);
                } else {
                    skipped += 1;
                    eprintln!(
                        "Could not stop expired container '{}' for '{}'.",
                        container, key
                    );
                }
            }
            Runtime::KubernetesPod(kubernetes) => {
                let pod_name =
                    berth::runtime::kubernetes::pod_name(&environment.workspace, &kubernetes);
                let spec = berth::runtime::kubernetes::build_delete_command(
                    &berth::runtime::kubernetes::KubernetesDeleteConfig::new(
                        &environment.workspace,
                        kubernetes.clone(),
                    ),
                )?;

                let status = runtime::run_command(&spec)?;
                if status.success() {
                    state.environments.remove(&key);
                    stopped += 1;
                    println!("Deleted expired pod '{}'.", pod_name);
                } else {
                    skipped += 1;
                    eprintln!("Could not delete expired pod '{}' for '{}'.", pod_name, key);
                }
            }
            Runtime::Auto => {
                skipped += 1;
                eprintln!("Skipping '{}': auto runtime was not resolved.", key);
            }
        }
    }

    lifecycle_state::save(&state)?;

    Ok(Summary { stopped, skipped })
}

pub fn print_summary(summary: Summary) {
    if summary.stopped == 0 && summary.skipped == 0 {
        println!("No expired local runtime environments found.");
    } else {
        println!(
            "Reaped {} environment(s), skipped {}.",
            summary.stopped, summary.skipped
        );
    }
}

fn should_reap(environment: &Environment, now: u64) -> bool {
    environment.is_local()
        && matches!(environment.runtime.as_str(), "podman" | "kubernetes-pod")
        && environment.is_expired_at(now)
}

fn podman_container_name(workspace: &str) -> String {
    format!("berth-{}", workspace.replace('/', "-"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use berth::lifecycle_state::Environment;

    #[test]
    fn reaps_only_expired_local_supported_ephemeral_environments() {
        let expired = Environment {
            workspace: "expired".to_string(),
            host: None,
            runtime: "podman".to_string(),
            last_active_epoch_seconds: 10,
            idle_shutdown_after_seconds: Some(5),
        };
        let active = Environment {
            workspace: "active".to_string(),
            host: None,
            runtime: "podman".to_string(),
            last_active_epoch_seconds: 10,
            idle_shutdown_after_seconds: Some(50),
        };
        let remote = Environment {
            workspace: "remote".to_string(),
            host: Some("host".to_string()),
            runtime: "podman".to_string(),
            last_active_epoch_seconds: 10,
            idle_shutdown_after_seconds: Some(5),
        };
        let bare = Environment {
            workspace: "bare".to_string(),
            host: None,
            runtime: "bare".to_string(),
            last_active_epoch_seconds: 10,
            idle_shutdown_after_seconds: Some(5),
        };
        let kubernetes = Environment {
            workspace: "pod".to_string(),
            host: None,
            runtime: "kubernetes-pod".to_string(),
            last_active_epoch_seconds: 10,
            idle_shutdown_after_seconds: Some(5),
        };

        assert!(should_reap(&expired, 15));
        assert!(should_reap(&kubernetes, 15));
        assert!(!should_reap(&active, 15));
        assert!(!should_reap(&remote, 15));
        assert!(!should_reap(&bare, 15));
    }

    #[test]
    fn podman_container_names_are_workspace_safe() {
        assert_eq!(podman_container_name("org/project"), "berth-org-project");
    }
}
