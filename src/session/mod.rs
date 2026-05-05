pub mod client;
pub mod protocol;
pub mod supervisor;

use anyhow::{Context, Result};
use std::path::PathBuf;

pub fn runtime_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("BERTH_RUNTIME_DIR") {
        return Ok(PathBuf::from(dir));
    }
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return Ok(PathBuf::from(dir).join("berth"));
    }
    let uid = unsafe { libc::getuid() };
    let candidate = PathBuf::from(format!("/run/user/{}", uid));
    if candidate.is_dir() {
        return Ok(candidate.join("berth"));
    }
    let home = dirs::home_dir().context("no home directory")?;
    Ok(home.join(".local").join("state").join("berth"))
}

pub fn session_socket(workspace: &str) -> Result<PathBuf> {
    let dir = runtime_dir()?.join("sessions");
    Ok(dir.join(format!("{}.sock", sanitize(workspace))))
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_unsafe_chars() {
        assert_eq!(sanitize("team/proj-1"), "team-proj-1");
        assert_eq!(sanitize("../etc"), "---etc");
    }

    #[test]
    fn session_socket_uses_sanitized_name() {
        std::env::set_var("BERTH_RUNTIME_DIR", "/tmp/berth-test-sess");
        let path = session_socket("team/proj").expect("socket path");
        assert!(path.ends_with("sessions/team-proj.sock"), "got {:?}", path);
        std::env::remove_var("BERTH_RUNTIME_DIR");
    }
}
