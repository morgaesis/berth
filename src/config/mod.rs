#![allow(clippy::derivable_impls)]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub workspaces: HashMap<String, Workspace>,
    #[serde(default)]
    pub defaults: Defaults,
    /// Hosts the user has authorized berth to deploy its own binary to.
    /// Persisted on first successful deploy; consulted by `berth enter`
    /// to decide whether to silently redeploy a stale remote.
    #[serde(default)]
    pub trusted_hosts: HashMap<String, TrustedHost>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrustedHost {
    pub target: String,
    pub version: String,
    pub remote_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub path: String,
    #[serde(default)]
    pub remote: Option<String>,
    #[serde(default)]
    pub ports: Option<Vec<u16>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<Runtime>,
    #[serde(default)]
    pub mounts: Vec<Mount>,
    #[serde(default)]
    pub idle: Idle,
    #[serde(default)]
    pub remote_options: RemoteOptions,
}

impl Workspace {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            remote: None,
            ports: None,
            runtime: None,
            mounts: Vec::new(),
            idle: Idle::default(),
            remote_options: RemoteOptions::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Defaults {
    #[serde(default)]
    pub runtime: Runtime,
    #[serde(default)]
    pub mounts: Vec<Mount>,
    #[serde(default)]
    pub idle: Idle,
    #[serde(default)]
    pub remote_options: RemoteOptions,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            runtime: Runtime::default(),
            mounts: Vec::new(),
            idle: Idle::default(),
            remote_options: RemoteOptions::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Runtime {
    Auto,
    Bare,
    Podman(PodmanRuntime),
    KubernetesPod(KubernetesPodRuntime),
}

impl Default for Runtime {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PodmanRuntime {
    #[serde(default = "default_podman_binary")]
    pub binary: String,
    #[serde(default = "default_podman_image")]
    pub image: String,
    #[serde(default)]
    pub pull: PullPolicy,
    #[serde(default)]
    pub ephemeral: bool,
    #[serde(default = "default_project_mount")]
    pub project_mount: String,
    #[serde(default)]
    pub userns: Option<String>,
    #[serde(default)]
    pub extra_args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KubernetesPodRuntime {
    #[serde(default = "default_kubectl_binary")]
    pub kubectl: String,
    #[serde(default = "default_kubernetes_image")]
    pub image: String,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub pod_name: Option<String>,
    #[serde(default)]
    pub container: Option<String>,
    #[serde(default = "default_project_mount")]
    pub project_mount: String,
    #[serde(default)]
    pub ephemeral: bool,
    #[serde(default)]
    pub extra_args: Vec<String>,
}

impl Default for KubernetesPodRuntime {
    fn default() -> Self {
        Self {
            kubectl: default_kubectl_binary(),
            image: default_kubernetes_image(),
            namespace: None,
            pod_name: None,
            container: None,
            project_mount: default_project_mount(),
            ephemeral: false,
            extra_args: Vec::new(),
        }
    }
}

impl Default for PodmanRuntime {
    fn default() -> Self {
        Self {
            binary: default_podman_binary(),
            image: default_podman_image(),
            pull: PullPolicy::default(),
            ephemeral: false,
            project_mount: default_project_mount(),
            userns: None,
            extra_args: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PullPolicy {
    Missing,
    Always,
    Never,
}

impl Default for PullPolicy {
    fn default() -> Self {
        Self::Missing
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Mount {
    pub source: String,
    pub target: String,
    #[serde(default = "default_mount_readonly")]
    pub readonly: bool,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Idle {
    #[serde(default)]
    pub shutdown_after_seconds: Option<u64>,
    #[serde(default)]
    pub action: IdleAction,
}

impl Default for Idle {
    fn default() -> Self {
        Self {
            shutdown_after_seconds: crate::discovery::default_idle_shutdown_seconds(),
            action: IdleAction::StopRuntime,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum IdleAction {
    StopRuntime,
    StopHost,
}

impl Default for IdleAction {
    fn default() -> Self {
        Self::StopRuntime
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteOptions {
    #[serde(default)]
    pub project_root: Option<String>,
    #[serde(default)]
    pub persistent: PersistentMode,
}

impl Default for RemoteOptions {
    fn default() -> Self {
        Self {
            project_root: None,
            persistent: PersistentMode::Auto,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PersistentMode {
    Auto,
    None,
    Tmux,
    Screen,
}

impl Default for PersistentMode {
    fn default() -> Self {
        Self::Auto
    }
}

fn default_podman_binary() -> String {
    "podman".to_string()
}

fn default_podman_image() -> String {
    "docker.io/library/debian:stable-slim".to_string()
}

fn default_kubectl_binary() -> String {
    "kubectl".to_string()
}

fn default_kubernetes_image() -> String {
    "docker.io/library/debian:stable-slim".to_string()
}

fn default_project_mount() -> String {
    "/workspace".to_string()
}

fn default_mount_readonly() -> bool {
    true
}

impl Config {
    pub fn load() -> Result<Self> {
        let config_dir = Self::config_dir()?;
        fs::create_dir_all(&config_dir)?;

        let yaml_path = config_dir.join(super::BERTH_CONFIG_FILE_YAML);
        let json_path = config_dir.join(super::BERTH_CONFIG_FILE_JSON);

        if yaml_path.exists() {
            let content = fs::read_to_string(&yaml_path)?;
            Ok(serde_yaml::from_str(&content)?)
        } else if json_path.exists() {
            let content = fs::read_to_string(&json_path)?;
            Ok(serde_json::from_str(&content)?)
        } else {
            Ok(Config {
                workspaces: HashMap::new(),
                defaults: Defaults::default(),
                trusted_hosts: HashMap::new(),
            })
        }
    }

    pub fn save(&self) -> Result<()> {
        let config_dir = Self::config_dir()?;
        fs::create_dir_all(&config_dir)?;

        let yaml_path = config_dir.join(super::BERTH_CONFIG_FILE_YAML);
        let yaml_content = serde_yaml::to_string(self)?;
        fs::write(&yaml_path, yaml_content)?;

        Ok(())
    }

    pub fn merged_runtime(&self, workspace: &Workspace) -> Runtime {
        self.merged_runtime_for(workspace, workspace.remote.is_some())
    }

    pub fn merged_runtime_for(&self, workspace: &Workspace, remote: bool) -> Runtime {
        let runtime = workspace
            .runtime
            .clone()
            .unwrap_or_else(|| self.defaults.runtime.clone());

        match runtime {
            Runtime::Auto if remote => Runtime::Bare,
            Runtime::Auto => crate::discovery::default_local_runtime(),
            runtime => runtime,
        }
    }

    pub fn merged_mounts(&self, workspace: &Workspace) -> Vec<Mount> {
        let mut mounts = self.defaults.mounts.clone();
        mounts.extend(workspace.mounts.clone());
        mounts
    }

    pub fn merged_idle(&self, workspace: &Workspace) -> Idle {
        if workspace.idle.shutdown_after_seconds.is_some() {
            workspace.idle.clone()
        } else {
            self.defaults.idle.clone()
        }
    }

    pub fn config_dir() -> Result<PathBuf> {
        if let Ok(dir) = env::var("BERTH_CONFIG_DIR") {
            return Ok(PathBuf::from(dir));
        }
        if let Ok(dir) = env::var("XDG_CONFIG_HOME") {
            return Ok(PathBuf::from(dir).join(super::BERTH_DIR));
        }

        dirs::config_dir()
            .map(|p| p.join(super::BERTH_DIR))
            .ok_or_else(|| anyhow::anyhow!("Cannot determine config directory"))
    }
}
