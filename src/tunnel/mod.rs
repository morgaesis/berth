use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TunnelState {
    pub tunnels: HashMap<String, Vec<u16>>,
}

impl TunnelState {
    pub fn load() -> Self {
        let path = Self::path();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(state) = serde_json::from_str(&content) {
                    return state;
                }
            }
        }
        Self::default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        fs::write(path, content)?;
        Ok(())
    }

    fn path() -> PathBuf {
        let data_dir = env::var("BERTH_DATA_DIR")
            .or_else(|_| env::var("XDG_DATA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("~/.local/share"))
            });
        data_dir.join("berth").join("tunnels.json")
    }

    pub fn add(&mut self, workspace: &str, ports: &[u16]) {
        let entry = self.tunnels.entry(workspace.to_string()).or_default();
        for port in ports {
            if !entry.contains(port) {
                entry.push(*port);
            }
        }
    }

    pub fn has_port(&self, workspace: &str, port: u16) -> bool {
        self.tunnels
            .get(workspace)
            .map(|ports| ports.contains(&port))
            .unwrap_or(false)
    }

    pub fn remove_port(&mut self, workspace: &str, port: u16) {
        if let Some(ports) = self.tunnels.get_mut(workspace) {
            ports.retain(|p| *p != port);
            if ports.is_empty() {
                self.tunnels.remove(workspace);
            }
        }
    }

    pub fn remove_workspace(&mut self, workspace: &str) {
        self.tunnels.remove(workspace);
    }
}
