pub mod config;
pub mod hosts;
pub mod ssh;
pub mod tunnel;

pub const BERTH_DIR: &str = "berth";
pub const BERTH_CONFIG_FILE_YAML: &str = "config.yaml";
pub const BERTH_CONFIG_FILE_JSON: &str = "config.json";
pub const BERTH_PROJECTS_DIR: &str = "projects";
pub const BERTH_SOCKET_NAME: &str = "berth.sock";

pub fn validate_workspace_name(name: &str) -> anyhow::Result<()> {
    let slash_count = name.matches('/').count();
    if slash_count > 1 {
        anyhow::bail!(
            "Invalid workspace name '{}': only one slash allowed (org/project format)",
            name
        );
    }
    if name.starts_with('/') || name.ends_with('/') {
        anyhow::bail!(
            "Invalid workspace name '{}': cannot start or end with slash",
            name
        );
    }
    if name.contains("//") {
        anyhow::bail!(
            "Invalid workspace name '{}': cannot contain double slashes",
            name
        );
    }
    if name.is_empty() || name == "/" {
        anyhow::bail!("Invalid workspace name: cannot be empty");
    }
    Ok(())
}
