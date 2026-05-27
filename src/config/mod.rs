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
    /// Per-org defaults. Workspace names of the form `<org>/<project>`
    /// look up their org here. `remote_root` lets you say "everything
    /// under `org/*` lives under `~/code/org/` on the
    /// remote", so individual workspaces don't have to repeat the path
    /// prefix. `remote` provides a default host for the org.
    #[serde(default)]
    pub orgs: HashMap<String, Org>,
    /// When the local berth is strictly newer than what's on a *trusted*
    /// remote, redeploy silently before entering. Defaults to true so
    /// trusted hosts stay in lockstep with the local install. Set to
    /// false to be told about the diff (via the version banner) but
    /// never have berth touch the remote without an explicit
    /// `berth deploy --force`.
    #[serde(default = "default_auto_update_remote")]
    pub auto_update_remote: bool,
}

fn default_auto_update_remote() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Org {
    /// Filesystem root on the remote. Final workspace dir is
    /// `<remote_root>/<project>` unless the workspace overrides it.
    /// Plain string; `$HOME`, `~`, etc. are expanded by the remote shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_root: Option<String>,
    /// Default host for any workspace in this org that doesn't set its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// Default SSH user for org workspaces. Applied when the effective
    /// host does not already include `user@`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_user: Option<String>,
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
    /// Override the remote working directory.
    ///
    /// When unset, the entry uses the auto-managed
    /// `$HOME/.local/share/berth/projects/<name>` path. When set, both
    /// kinds of paths are passed to `mkdir -p` + `cd` so a fresh workspace
    /// dir is created on first use; missing directories are not treated
    /// as an error.
    ///
    /// Allows `$HOME`, `~`, and other remote-shell expansions verbatim
    /// because the string is wrapped in double quotes when interpolated,
    /// not single-quoted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_dir: Option<String>,
    /// Default command for `berth attach --new`. When unset, the
    /// supervisor spawns `$SHELL -l`. When set, this argv replaces it
    /// — e.g. `["claude", "--dangerously-skip-permissions"]` to land
    /// straight in claude inside the workspace dir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
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
            remote_dir: None,
            command: None,
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
    /// Whether the new-tab shell hook is allowed to auto-enter a
    /// workspace when a fresh shell starts inside the marker dir. True
    /// by default; set to false to disable replay even with the hook
    /// installed (without having to edit your rc).
    #[serde(default = "default_new_tab_auto_entry")]
    pub new_tab_auto_entry: bool,
    /// Key sequence that detaches the current berth attach client
    /// without stopping the supervised session. Set to null to disable.
    #[serde(
        default = "default_detach_key",
        skip_serializing_if = "Option::is_none"
    )]
    pub detach_key: Option<String>,
}

fn default_new_tab_auto_entry() -> bool {
    true
}

fn default_detach_key() -> Option<String> {
    Some("ctrl-]".to_string())
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            runtime: Runtime::default(),
            mounts: Vec::new(),
            idle: Idle::default(),
            remote_options: RemoteOptions::default(),
            new_tab_auto_entry: default_new_tab_auto_entry(),
            detach_key: default_detach_key(),
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
                orgs: HashMap::new(),
                auto_update_remote: true,
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

    pub fn detach_key_bytes(&self) -> Result<Option<Vec<u8>>> {
        if let Ok(value) = env::var("BERTH_DETACH_KEY") {
            return parse_detach_key(Some(value.as_str()));
        }
        self.detach_key_bytes_for_remote()
    }

    pub fn detach_key_bytes_for_remote(&self) -> Result<Option<Vec<u8>>> {
        parse_detach_key(self.defaults.detach_key.as_deref())
    }

    pub fn detach_key_env_value(&self) -> String {
        self.defaults
            .detach_key
            .clone()
            .unwrap_or_else(|| "none".to_string())
    }

    /// Resolve the effective remote directory for a workspace, in order:
    ///   1. workspace.remote_dir (explicit override on the workspace)
    ///   2. `<orgs[<org>].remote_root>/<project>` if the workspace name
    ///      is `<org>/<project>` and that org has a `remote_root`
    ///   3. None — the caller should fall back to the auto-managed path
    ///      under `$HOME/.local/share/berth/projects/<name>`.
    pub fn resolved_remote_dir(
        &self,
        workspace_name: &str,
        workspace: &Workspace,
    ) -> Option<String> {
        if let Some(dir) = &workspace.remote_dir {
            return Some(dir.clone());
        }
        let (org, project) = workspace_name.split_once('/')?;
        let org_cfg = self.orgs.get(org)?;
        let root = org_cfg.remote_root.as_deref()?;
        let root = root.trim_end_matches('/');
        Some(format!("{root}/{project}"))
    }

    /// Resolve the effective remote host for a workspace, in order:
    ///   1. CLI `--remote` (handled by the caller)
    ///   2. workspace.remote
    ///   3. orgs[<org>].remote if the workspace name is `<org>/<project>`
    ///   4. orgs[<org>].remote_user if the host has no explicit user
    pub fn resolved_remote(&self, workspace_name: &str, workspace: &Workspace) -> Option<String> {
        let org_cfg = workspace_name
            .split_once('/')
            .and_then(|(org, _project)| self.orgs.get(org));
        let host = workspace
            .remote
            .as_ref()
            .or_else(|| org_cfg.and_then(|org| org.remote.as_ref()))?;
        Some(apply_remote_user(
            host,
            org_cfg.and_then(|org| org.remote_user.as_deref()),
        ))
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

fn apply_remote_user(host: &str, user: Option<&str>) -> String {
    let Some(user) = user.filter(|u| !u.trim().is_empty()) else {
        return host.to_string();
    };
    if host.contains('@') {
        host.to_string()
    } else {
        format!("{user}@{host}")
    }
}

fn parse_detach_key(value: Option<&str>) -> Result<Option<Vec<u8>>> {
    let Some(raw) = value else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("none")
        || trimmed.eq_ignore_ascii_case("off")
        || trimmed.eq_ignore_ascii_case("disabled")
    {
        return Ok(None);
    }

    let lower = trimmed.to_ascii_lowercase();
    if lower == "esc" || lower == "escape" {
        return Ok(Some(vec![0x1b]));
    }
    if let Some(rest) = lower
        .strip_prefix("ctrl-")
        .or_else(|| lower.strip_prefix("ctrl+"))
        .or_else(|| lower.strip_prefix("c-"))
        .or_else(|| lower.strip_prefix("c+"))
    {
        let byte = ctrl_byte(rest).ok_or_else(|| {
            anyhow::anyhow!(
                "invalid defaults.detach_key '{trimmed}'; expected ctrl-A..ctrl-Z, ctrl-], esc, or null"
            )
        })?;
        return Ok(Some(vec![byte]));
    }

    if trimmed.len() == 1 && trimmed.is_ascii() {
        return Ok(Some(trimmed.as_bytes().to_vec()));
    }

    anyhow::bail!(
        "invalid defaults.detach_key '{trimmed}'; expected ctrl-A..ctrl-Z, ctrl-], esc, a single ASCII character, or null"
    )
}

fn ctrl_byte(value: &str) -> Option<u8> {
    let mut chars = value.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    match c {
        'a'..='z' => Some((c as u8) - b'a' + 1),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        '?' => Some(0x7f),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with_org(name: &str, remote_root: Option<&str>, remote: Option<&str>) -> Config {
        let mut c = Config {
            workspaces: HashMap::new(),
            defaults: Defaults::default(),
            trusted_hosts: HashMap::new(),
            orgs: HashMap::new(),
            auto_update_remote: true,
        };
        c.orgs.insert(
            name.into(),
            Org {
                remote_root: remote_root.map(Into::into),
                remote: remote.map(Into::into),
                remote_user: None,
            },
        );
        c
    }

    #[test]
    fn resolved_remote_dir_workspace_override_wins() {
        let cfg = cfg_with_org("org", Some("~/code/org"), None);
        let mut ws = Workspace::new("/tmp/x");
        ws.remote_dir = Some("~/elsewhere/proj".into());
        assert_eq!(
            cfg.resolved_remote_dir("org/proj", &ws),
            Some("~/elsewhere/proj".into())
        );
    }

    #[test]
    fn resolved_remote_dir_uses_org_root() {
        let cfg = cfg_with_org("org", Some("~/code/org"), None);
        let ws = Workspace::new("/tmp/x");
        assert_eq!(
            cfg.resolved_remote_dir("org/proj", &ws),
            Some("~/code/org/proj".into())
        );
    }

    #[test]
    fn resolved_remote_dir_trims_trailing_slash_on_root() {
        let cfg = cfg_with_org("org", Some("~/code/org/"), None);
        let ws = Workspace::new("/tmp/x");
        assert_eq!(
            cfg.resolved_remote_dir("org/proj", &ws),
            Some("~/code/org/proj".into())
        );
    }

    #[test]
    fn resolved_remote_dir_returns_none_for_unscoped_name() {
        let cfg = cfg_with_org("org", Some("~/p"), None);
        let ws = Workspace::new("/tmp/x");
        assert_eq!(cfg.resolved_remote_dir("proj", &ws), None);
    }

    #[test]
    fn resolved_remote_dir_returns_none_when_org_unknown() {
        let cfg = cfg_with_org("org", Some("~/p"), None);
        let ws = Workspace::new("/tmp/x");
        assert_eq!(cfg.resolved_remote_dir("other/proj", &ws), None);
    }

    #[test]
    fn resolved_remote_uses_workspace_first_then_org() {
        let cfg = cfg_with_org("org", None, Some("dev-box"));
        let mut ws = Workspace::new("/tmp/x");
        assert_eq!(cfg.resolved_remote("org/proj", &ws), Some("dev-box".into()));
        ws.remote = Some("personal-box".into());
        assert_eq!(
            cfg.resolved_remote("org/proj", &ws),
            Some("personal-box".into())
        );
    }

    #[test]
    fn resolved_remote_applies_org_user_to_bare_host() {
        let mut cfg = cfg_with_org("org", None, Some("dev-box"));
        cfg.orgs.get_mut("org").unwrap().remote_user = Some("alice".into());
        let ws = Workspace::new("/tmp/x");
        assert_eq!(
            cfg.resolved_remote("org/proj", &ws),
            Some("alice@dev-box".into())
        );
    }

    #[test]
    fn resolved_remote_does_not_override_explicit_host_user() {
        let mut cfg = cfg_with_org("org", None, Some("dev-box"));
        cfg.orgs.get_mut("org").unwrap().remote_user = Some("alice".into());
        let mut ws = Workspace::new("/tmp/x");
        ws.remote = Some("bob@other-box".into());
        assert_eq!(
            cfg.resolved_remote("org/proj", &ws),
            Some("bob@other-box".into())
        );
    }

    #[test]
    fn resolved_remote_keeps_unscoped_workspace_remote() {
        let cfg = Config {
            workspaces: HashMap::new(),
            defaults: Defaults::default(),
            trusted_hosts: HashMap::new(),
            orgs: HashMap::new(),
            auto_update_remote: true,
        };
        let mut ws = Workspace::new("/tmp/x");
        ws.remote = Some("dev-box".into());
        assert_eq!(cfg.resolved_remote("proj", &ws), Some("dev-box".into()));
    }

    #[test]
    fn default_detach_key_is_ctrl_bracket() {
        let cfg = Config {
            workspaces: HashMap::new(),
            defaults: Defaults::default(),
            trusted_hosts: HashMap::new(),
            orgs: HashMap::new(),
            auto_update_remote: true,
        };
        assert_eq!(cfg.detach_key_bytes().unwrap(), Some(vec![0x1d]));
    }

    #[test]
    fn detach_key_can_be_disabled() {
        assert_eq!(parse_detach_key(None).unwrap(), None);
        assert_eq!(parse_detach_key(Some("none")).unwrap(), None);
    }

    #[test]
    fn detach_key_parses_common_control_names() {
        assert_eq!(parse_detach_key(Some("ctrl-a")).unwrap(), Some(vec![1]));
        assert_eq!(parse_detach_key(Some("ctrl-]")).unwrap(), Some(vec![0x1d]));
        assert_eq!(parse_detach_key(Some("esc")).unwrap(), Some(vec![0x1b]));
    }
}
