pub mod client;
pub mod protocol;
pub mod supervisor;

use anyhow::{Context, Result};
use std::fs;
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

/// Directory holding all supervisor sockets for a workspace. Each
/// supervisor owns one `<session_id>.sock` file inside this directory.
pub fn sessions_dir(workspace: &str) -> Result<PathBuf> {
    Ok(runtime_dir()?.join("sessions").join(sanitize(workspace)))
}

/// Socket path for a specific session within a workspace.
pub fn session_socket(workspace: &str, session_id: &str) -> Result<PathBuf> {
    Ok(sessions_dir(workspace)?.join(format!("{}.sock", sanitize(session_id))))
}

/// Enumerate live sessions for a workspace by listing `.sock` files. Stale
/// entries (no listener) are reported just like live ones; the caller should
/// probe before attaching if it cares.
pub fn list_sessions(workspace: &str) -> Result<Vec<String>> {
    let dir = sessions_dir(workspace)?;
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut out = Vec::new();
    for entry in
        fs::read_dir(&dir).with_context(|| format!("reading sessions dir {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("sock") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                out.push(stem.to_string());
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Short, unique session identifier (12 lowercase hex chars). Generated
/// client-side and passed to the supervisor so the client knows which
/// socket file to wait for.
pub fn new_session_id() -> String {
    let raw = uuid::Uuid::new_v4().simple().to_string();
    raw.chars().take(12).collect()
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // BERTH_RUNTIME_DIR is process-global; tests that mutate it must
    // serialize via this mutex to avoid clobbering each other under the
    // default parallel test runner.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn sanitize_replaces_unsafe_chars() {
        assert_eq!(sanitize("team/proj-1"), "team-proj-1");
        assert_eq!(sanitize("../etc"), "---etc");
    }

    #[test]
    fn session_socket_partitions_by_workspace_and_id() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = tempdir();
        std::env::set_var("BERTH_RUNTIME_DIR", &tmp);
        let path = session_socket("team/proj", "abc123").expect("socket path");
        assert!(
            path.ends_with("sessions/team-proj/abc123.sock"),
            "got {:?}",
            path
        );
        std::env::remove_var("BERTH_RUNTIME_DIR");
    }

    #[test]
    fn new_session_id_is_short_and_unique() {
        let a = new_session_id();
        let b = new_session_id();
        assert_eq!(a.len(), 12);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn list_sessions_empty_when_no_dir() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = tempdir();
        std::env::set_var("BERTH_RUNTIME_DIR", &tmp);
        let ids = list_sessions("ghost").expect("list");
        assert!(ids.is_empty());
        std::env::remove_var("BERTH_RUNTIME_DIR");
    }

    #[test]
    fn list_sessions_enumerates_sock_files_sorted() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = tempdir();
        std::env::set_var("BERTH_RUNTIME_DIR", &tmp);
        let dir = sessions_dir("ws").expect("sessions dir");
        fs::create_dir_all(&dir).unwrap();
        for id in ["bbb", "aaa", "ccc"] {
            fs::File::create(dir.join(format!("{id}.sock"))).unwrap();
        }
        // non-.sock entries must be ignored.
        fs::File::create(dir.join("README")).unwrap();

        let ids = list_sessions("ws").expect("list");
        assert_eq!(ids, vec!["aaa", "bbb", "ccc"]);
        std::env::remove_var("BERTH_RUNTIME_DIR");
    }

    fn tempdir() -> String {
        let pid = std::process::id();
        let nonce = new_session_id();
        let path = format!("/tmp/berth-test-{}-{}", pid, nonce);
        fs::create_dir_all(&path).unwrap();
        path
    }
}
