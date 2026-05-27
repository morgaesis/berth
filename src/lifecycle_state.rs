use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct State {
    pub environments: HashMap<String, Environment>,
    #[serde(default)]
    pub sessions: HashMap<String, SessionStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    pub workspace: String,
    pub host: Option<String>,
    pub runtime: String,
    pub last_active_epoch_seconds: u64,
    pub idle_shutdown_after_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStatus {
    pub workspace: String,
    pub host: Option<String>,
    pub refreshed_epoch_seconds: u64,
    pub live_sessions: usize,
    pub attached_sessions: usize,
    pub exited_sessions: usize,
}

impl Environment {
    pub fn is_local(&self) -> bool {
        self.host.is_none()
    }

    pub fn is_expired_at(&self, now_epoch_seconds: u64) -> bool {
        self.idle_shutdown_after_seconds
            .filter(|ttl| *ttl > 0)
            .and_then(|ttl| self.last_active_epoch_seconds.checked_add(ttl))
            .map(|expires_at| now_epoch_seconds >= expires_at)
            .unwrap_or(false)
    }
}

pub fn touch(
    workspace: &str,
    host: Option<&str>,
    runtime: &str,
    idle_shutdown_after_seconds: Option<u64>,
) -> std::io::Result<()> {
    let mut state = load();
    state.environments.insert(
        key(workspace, host),
        Environment {
            workspace: workspace.to_string(),
            host: host.map(str::to_string),
            runtime: runtime.to_string(),
            last_active_epoch_seconds: now(),
            idle_shutdown_after_seconds,
        },
    );
    save(&state)
}

pub fn remove(workspace: &str, host: Option<&str>) -> std::io::Result<()> {
    let mut state = load();
    state.environments.remove(&key(workspace, host));
    save(&state)
}

pub fn update_session_status(
    workspace: &str,
    host: Option<&str>,
    live_sessions: usize,
    attached_sessions: usize,
    exited_sessions: usize,
) -> std::io::Result<()> {
    let mut state = load();
    state.sessions.insert(
        key(workspace, host),
        SessionStatus {
            workspace: workspace.to_string(),
            host: host.map(str::to_string),
            refreshed_epoch_seconds: now(),
            live_sessions,
            attached_sessions,
            exited_sessions,
        },
    );
    save(&state)
}

pub fn session_status<'a>(
    state: &'a State,
    workspace: &str,
    host: Option<&str>,
) -> Option<&'a SessionStatus> {
    state.sessions.get(&key(workspace, host))
}

pub fn load() -> State {
    let path = path();
    if let Ok(content) = fs::read_to_string(path) {
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        State::default()
    }
}

pub fn save(state: &State) -> std::io::Result<()> {
    let path = path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_string_pretty(state)?)
}

fn path() -> PathBuf {
    let data_dir = env::var("BERTH_DATA_DIR")
        .or_else(|_| env::var("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs::data_local_dir().unwrap_or_else(|| PathBuf::from(".cache")));
    data_dir.join("berth").join("lifecycle.json")
}

fn key(workspace: &str, host: Option<&str>) -> String {
    match host {
        Some(host) => format!("{}@{}", workspace, host),
        None => workspace.to_string(),
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::Environment;

    #[test]
    fn environment_expiration_requires_positive_ttl() {
        let mut environment = Environment {
            workspace: "project".to_string(),
            host: None,
            runtime: "podman".to_string(),
            last_active_epoch_seconds: 100,
            idle_shutdown_after_seconds: None,
        };

        assert!(!environment.is_expired_at(200));

        environment.idle_shutdown_after_seconds = Some(0);
        assert!(!environment.is_expired_at(200));

        environment.idle_shutdown_after_seconds = Some(60);
        assert!(!environment.is_expired_at(159));
        assert!(environment.is_expired_at(160));
    }
}
