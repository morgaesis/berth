use crate::config::KubernetesPodRuntime;

use super::{validate_command, CommandSpec, RuntimeCommandError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KubernetesRunConfig {
    pub workspace: String,
    pub runtime: KubernetesPodRuntime,
    pub command: Vec<String>,
}

impl KubernetesRunConfig {
    pub fn new(
        workspace: impl Into<String>,
        runtime: KubernetesPodRuntime,
        command: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            workspace: workspace.into(),
            runtime,
            command: command.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KubernetesDeleteConfig {
    pub workspace: String,
    pub runtime: KubernetesPodRuntime,
}

impl KubernetesDeleteConfig {
    pub fn new(workspace: impl Into<String>, runtime: KubernetesPodRuntime) -> Self {
        Self {
            workspace: workspace.into(),
            runtime,
        }
    }
}

pub fn build_run_command(config: &KubernetesRunConfig) -> Result<CommandSpec, RuntimeCommandError> {
    if config.runtime.image.trim().is_empty() {
        return Err(RuntimeCommandError::EmptyImage);
    }
    validate_command(&config.command)?;

    let pod_name = pod_name(&config.workspace, &config.runtime);
    let mut args = vec![
        "run".to_string(),
        pod_name,
        "--image".to_string(),
        config.runtime.image.clone(),
        "--restart".to_string(),
        "Never".to_string(),
        "--attach".to_string(),
        "--rm".to_string(),
    ];

    if let Some(namespace) = &config.runtime.namespace {
        args.splice(2..2, ["--namespace".to_string(), namespace.clone()]);
    }

    args.extend(config.runtime.extra_args.clone());
    args.push("--command".to_string());
    args.push("--".to_string());
    args.extend(config.command.clone());

    Ok(CommandSpec::new(&config.runtime.kubectl).with_args(args))
}

pub fn build_delete_command(
    config: &KubernetesDeleteConfig,
) -> Result<CommandSpec, RuntimeCommandError> {
    let pod_name = pod_name(&config.workspace, &config.runtime);
    if pod_name.trim().is_empty() {
        return Err(RuntimeCommandError::EmptyCommand);
    }

    let mut args = vec!["delete".to_string(), "pod".to_string(), pod_name];
    if let Some(namespace) = &config.runtime.namespace {
        args.extend(["--namespace".to_string(), namespace.clone()]);
    }
    args.push("--ignore-not-found=true".to_string());

    Ok(CommandSpec::new(&config.runtime.kubectl).with_args(args))
}

pub fn pod_name(workspace: &str, runtime: &KubernetesPodRuntime) -> String {
    runtime
        .pod_name
        .clone()
        .unwrap_or_else(|| format!("berth-{}", workspace.replace('/', "-")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_kubectl_run_command() {
        let runtime = KubernetesPodRuntime {
            namespace: Some("dev".to_string()),
            pod_name: Some("berth-custom".to_string()),
            image: "alpine:latest".to_string(),
            ..KubernetesPodRuntime::default()
        };

        let command = build_run_command(&KubernetesRunConfig::new(
            "project",
            runtime,
            ["echo", "ok"],
        ))
        .unwrap();

        assert_eq!(command.program, "kubectl");
        assert_eq!(
            command.args,
            [
                "run",
                "berth-custom",
                "--namespace",
                "dev",
                "--image",
                "alpine:latest",
                "--restart",
                "Never",
                "--attach",
                "--rm",
                "--command",
                "--",
                "echo",
                "ok"
            ]
        );
    }

    #[test]
    fn builds_kubectl_delete_command() {
        let runtime = KubernetesPodRuntime {
            namespace: Some("dev".to_string()),
            pod_name: Some("berth-custom".to_string()),
            ..KubernetesPodRuntime::default()
        };

        let command =
            build_delete_command(&KubernetesDeleteConfig::new("project", runtime)).unwrap();

        assert_eq!(
            command.args,
            [
                "delete",
                "pod",
                "berth-custom",
                "--namespace",
                "dev",
                "--ignore-not-found=true"
            ]
        );
    }
}
