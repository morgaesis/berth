use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub workspaces: HashMap<String, Workspace>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub path: String,
    pub remote: Option<String>,
    pub ports: Option<Vec<u16>>,
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

    pub fn config_dir() -> Result<PathBuf> {
        dirs::config_dir()
            .map(|p| p.join(super::BERTH_CONFIG_DIR))
            .ok_or_else(|| anyhow::anyhow!("Cannot determine config directory"))
    }
}
