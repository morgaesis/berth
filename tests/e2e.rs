use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

struct TestContext {
    bin: PathBuf,
    config_dir: PathBuf,
    temp_dir: TempDir,
}

impl TestContext {
    fn new() -> Self {
        let bin = PathBuf::from(env!("CARGO_BIN_EXE_berth"));
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config_dir = temp_dir.path().join(".config").join("berth");
        fs::create_dir_all(&config_dir).expect("Failed to create config dir");

        Self {
            bin,
            config_dir,
            temp_dir,
        }
    }

    fn berth(&self) -> Command {
        let mut cmd = Command::new(&self.bin);
        cmd.env("BERTH_CONFIG_DIR", &self.config_dir);
        cmd.env("BERTH_SKIP_HOSTS", "1");
        cmd.env("BERTH_SKIP_SSH", "1");
        cmd.env("HOME", self.temp_dir.path());
        cmd
    }

    fn project_path(&self, name: &str) -> PathBuf {
        self.temp_dir.path().join("projects").join(name)
    }
}

#[test]
fn test_new_workspace() {
    let ctx = TestContext::new();
    let project_path = ctx.project_path("testproj");

    let output = ctx
        .berth()
        .args(["new", "testproj", project_path.to_str().unwrap()])
        .output()
        .expect("Failed to run berth new");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(project_path.exists(), "Project directory was not created");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Created workspace 'testproj'"));
}

#[test]
fn test_new_workspace_creates_directory() {
    let ctx = TestContext::new();
    let project_path = ctx.project_path("newproj");

    assert!(!project_path.exists(), "Project path should not exist yet");

    let output = ctx
        .berth()
        .args(["new", "newproj", project_path.to_str().unwrap()])
        .output()
        .expect("Failed to run berth new");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(project_path.exists(), "Project directory should be created");
}

#[test]
fn test_new_workspace_duplicate_fails() {
    let ctx = TestContext::new();
    let project_path = ctx.project_path("dupproj");

    let output1 = ctx
        .berth()
        .args(["new", "dupproj", project_path.to_str().unwrap()])
        .output()
        .expect("Failed to run berth new");
    assert!(output1.status.success());

    let output2 = ctx
        .berth()
        .args(["new", "dupproj", project_path.to_str().unwrap()])
        .output()
        .expect("Failed to run berth new");

    assert!(!output2.status.success(), "Duplicate workspace should fail");
    let stderr = String::from_utf8_lossy(&output2.stderr);
    assert!(
        stderr.contains("already exists"),
        "Error should mention 'already exists'"
    );
}

#[test]
fn test_list_workspaces() {
    let ctx = TestContext::new();

    let path1 = ctx.project_path("proj1");
    let path2 = ctx.project_path("proj2");

    ctx.berth()
        .args(["new", "proj1", path1.to_str().unwrap()])
        .output()
        .expect("Failed to create proj1");

    ctx.berth()
        .args(["new", "proj2", path2.to_str().unwrap()])
        .output()
        .expect("Failed to create proj2");

    let output = ctx
        .berth()
        .args(["list"])
        .output()
        .expect("Failed to run berth list");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("proj1"));
    assert!(stdout.contains("proj2"));
}

#[test]
fn test_list_empty() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args(["list"])
        .output()
        .expect("Failed to run berth list");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No workspaces"));
}

#[test]
fn test_delete_workspace() {
    let ctx = TestContext::new();
    let project_path = ctx.project_path("delproj");

    ctx.berth()
        .args(["new", "delproj", project_path.to_str().unwrap()])
        .output()
        .expect("Failed to create workspace");

    let list_output = ctx.berth().args(["list"]).output().expect("Failed to list");
    assert!(String::from_utf8_lossy(&list_output.stdout).contains("delproj"));

    let delete_output = ctx
        .berth()
        .args(["delete", "delproj"])
        .output()
        .expect("Failed to delete");
    assert!(delete_output.status.success());

    let list_output2 = ctx.berth().args(["list"]).output().expect("Failed to list");
    assert!(!String::from_utf8_lossy(&list_output2.stdout).contains("delproj"));
}

#[test]
fn test_delete_nonexistent_fails() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args(["delete", "nonexistent"])
        .output()
        .expect("Failed to run delete");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not found"));
}

#[test]
fn test_config_yaml_format() {
    let ctx = TestContext::new();
    let project_path = ctx.project_path("yamlproj");

    ctx.berth()
        .args(["new", "yamlproj", project_path.to_str().unwrap()])
        .output()
        .expect("Failed to create workspace");

    let config_path = ctx.config_dir.join("config.yaml");
    assert!(config_path.exists(), "YAML config should be created");

    let content = fs::read_to_string(&config_path).expect("Failed to read config");
    assert!(content.contains("yamlproj"));
    assert!(content.contains("path:"));
}

#[test]
fn test_new_with_remote() {
    let ctx = TestContext::new();
    let project_path = ctx.project_path("remoteproj");

    let output = ctx
        .berth()
        .args([
            "new",
            "remoteproj",
            project_path.to_str().unwrap(),
            "--remote",
            "user@host",
        ])
        .output()
        .expect("Failed to create remote workspace");

    assert!(output.status.success());

    let list_output = ctx.berth().args(["list"]).output().expect("Failed to list");

    let stdout = String::from_utf8_lossy(&list_output.stdout);
    assert!(stdout.contains("remoteproj"));
    assert!(stdout.contains("remote"));
}

#[test]
fn test_new_with_ports() {
    let ctx = TestContext::new();
    let project_path = ctx.project_path("portproj");

    let output = ctx
        .berth()
        .args([
            "new",
            "portproj",
            project_path.to_str().unwrap(),
            "--ports",
            "3000,8080",
        ])
        .output()
        .expect("Failed to create workspace with ports");

    assert!(output.status.success());

    let config_path = ctx.config_dir.join("config.yaml");
    let content = fs::read_to_string(&config_path).expect("Failed to read config");
    assert!(content.contains("3000"));
    assert!(content.contains("8080"));
}

#[test]
fn test_init_shell() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args(["init-shell"])
        .output()
        .expect("Failed to run init-shell");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("_berth_auto_enter"));
    assert!(stdout.contains("_berth_chpwd"));
    assert!(stdout.contains("_berth_set_title"));
}

#[test]
fn test_default_command_creates_and_enters() {
    let ctx = TestContext::new();
    let project_path = ctx.temp_dir.path().join("projects").join("newproj");

    assert!(!project_path.exists(), "Project should not exist yet");

    let list_before = ctx.berth().args(["list"]).output().expect("Failed to list");
    assert!(!String::from_utf8_lossy(&list_before.stdout).contains("newproj"));

    let output = ctx
        .berth()
        .args(["newproj"])
        .output()
        .expect("Failed to run berth newproj");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(project_path.exists(), "Project directory should be created");

    let list_after = ctx.berth().args(["list"]).output().expect("Failed to list");
    assert!(String::from_utf8_lossy(&list_after.stdout).contains("newproj"));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Created directory") || stdout.contains("Created workspace"));
}

#[test]
fn test_default_command_enters_existing() {
    let ctx = TestContext::new();
    let project_path = ctx.project_path("existingproj");

    ctx.berth()
        .args(["new", "existingproj", project_path.to_str().unwrap()])
        .output()
        .expect("Failed to create workspace");

    let output = ctx
        .berth()
        .args(["existingproj"])
        .output()
        .expect("Failed to run berth existingproj");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("Created workspace"),
        "Should not recreate existing"
    );
}

#[test]
fn test_default_command_with_remote() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args(["remotedefault", "--remote", "user@remotehost"])
        .output()
        .expect("Failed to run berth with remote");

    assert!(output.status.success());

    let config_path = ctx.config_dir.join("config.yaml");
    let content = fs::read_to_string(&config_path).expect("Failed to read config");
    assert!(content.contains("remotedefault"));
    assert!(content.contains("user@remotehost"));
}

#[test]
fn test_default_command_with_ports() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args(["portdefault", "--ports", "3000,8080,9000"])
        .output()
        .expect("Failed to run berth with ports");

    assert!(output.status.success());

    let config_path = ctx.config_dir.join("config.yaml");
    let content = fs::read_to_string(&config_path).expect("Failed to read config");
    assert!(content.contains("portdefault"));
    assert!(content.contains("3000"));
    assert!(content.contains("8080"));
    assert!(content.contains("9000"));
}

#[test]
fn test_default_command_recreates_missing_path() {
    let ctx = TestContext::new();
    let project_path = ctx.project_path("recreateproj");

    ctx.berth()
        .args(["new", "recreateproj", project_path.to_str().unwrap()])
        .output()
        .expect("Failed to create workspace");

    fs::remove_dir_all(&project_path).ok();
    assert!(!project_path.exists());

    let output = ctx
        .berth()
        .args(["recreateproj"])
        .output()
        .expect("Failed to run berth recreateproj");

    assert!(output.status.success());
    assert!(
        project_path.exists(),
        "Project directory should be recreated"
    );
}

#[test]
fn test_stop_local_workspace() {
    let ctx = TestContext::new();
    let project_path = ctx.project_path("stopproj");

    ctx.berth()
        .args(["new", "stopproj", project_path.to_str().unwrap()])
        .output()
        .expect("Failed to create workspace");

    let output = ctx
        .berth()
        .args(["stop", "stopproj"])
        .output()
        .expect("Failed to run stop");

    assert!(output.status.success());
}

#[test]
fn test_full_workflow() {
    let ctx = TestContext::new();

    let proj_path = ctx.project_path("workflow");

    let new_output = ctx
        .berth()
        .args(["new", "workflow", proj_path.to_str().unwrap()])
        .output()
        .expect("Failed to create");
    assert!(new_output.status.success());
    assert!(proj_path.exists());

    let list_output = ctx.berth().args(["list"]).output().expect("Failed to list");
    assert!(String::from_utf8_lossy(&list_output.stdout).contains("workflow"));

    let delete_output = ctx
        .berth()
        .args(["delete", "workflow"])
        .output()
        .expect("Failed to delete");
    assert!(delete_output.status.success());

    let list_output2 = ctx.berth().args(["list"]).output().expect("Failed to list");
    assert!(!String::from_utf8_lossy(&list_output2.stdout).contains("workflow"));
}

#[test]
fn test_no_args_shows_list() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args::<[&str; 0], &str>([])
        .output()
        .expect("Failed to run berth with no args");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No workspaces") || stdout.contains("NAME"));
}
