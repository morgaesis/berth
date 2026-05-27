use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TestContext {
    bin: PathBuf,
    config_dir: PathBuf,
    temp_dir: PathBuf,
}

impl TestContext {
    fn new() -> Self {
        let bin = PathBuf::from(env!("CARGO_BIN_EXE_berth"));
        let temp_dir = repo_test_dir();
        let config_dir = temp_dir.join(".config").join("berth");
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
        cmd.env("BERTH_AUTO_DISCOVERY", "0");
        cmd.env("HOME", &self.temp_dir);
        cmd.env("XDG_DATA_HOME", self.temp_dir.join(".local").join("share"));
        cmd
    }

    fn berth_with_auto_discovery(&self, fake_bin: &std::path::Path) -> Command {
        let mut cmd = self.berth();
        cmd.env_remove("BERTH_AUTO_DISCOVERY");
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = std::env::split_paths(&path).collect::<Vec<_>>();
        paths.insert(0, fake_bin.to_path_buf());
        let joined = std::env::join_paths(paths).expect("Failed to build test PATH");
        cmd.env("PATH", joined);
        cmd
    }

    fn berth_with_fake_exec(&self, log: &PathBuf) -> Command {
        let mut cmd = self.berth();
        cmd.env("BERTH_FAKE_EXEC_LOG", log);
        cmd
    }

    fn berth_with_host_container_runtime(&self) -> Command {
        let mut cmd = self.berth();
        if let Some(home) = std::env::var_os("HOME") {
            cmd.env("HOME", home);
        }
        cmd.env("BERTH_DATA_DIR", self.temp_dir.join(".local").join("share"));
        cmd.env_remove("XDG_DATA_HOME");
        cmd
    }

    fn project_path(&self, name: &str) -> PathBuf {
        self.temp_dir
            .join(".local")
            .join("share")
            .join("berth")
            .join("projects")
            .join(name)
    }

    fn data_dir(&self) -> PathBuf {
        self.temp_dir.join(".local").join("share")
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp_dir);
    }
}

fn repo_test_dir() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(".cache")
        .join("e2e");
    fs::create_dir_all(&root).expect("Failed to create repo-local e2e dir");

    let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = root.join(format!("{}-{}", std::process::id(), n));
    if path.exists() {
        fs::remove_dir_all(&path).expect("Failed to clear stale e2e dir");
    }
    fs::create_dir_all(&path).expect("Failed to create repo-local test dir");
    path
}

/// Real-runtime e2e runs by default. The opt-out lives in a `.env` file
/// at the repo root (gitignored) so each contributor's machine carries
/// its own truth — a dev box without a kubeconfig sets
/// `BERTH_E2E_K8S_ENABLED=false` locally; a box without podman sets
/// `BERTH_E2E_PODMAN_ENABLED=false`. CI leaves both unset and runs the
/// full suite. See `.env.example` for the documented vars.
fn ensure_dotenv_loaded() {
    use std::sync::Once;
    static LOAD: Once = Once::new();
    LOAD.call_once(|| {
        // dotenvy::dotenv() returns Err when no .env exists — that's the
        // expected case on most contributor machines, not a failure.
        let _ = dotenvy::dotenv();
    });
}

fn real_podman_e2e_enabled() -> bool {
    ensure_dotenv_loaded();
    !is_disabled_env("BERTH_E2E_PODMAN_ENABLED")
}

fn real_k8s_e2e_enabled() -> bool {
    ensure_dotenv_loaded();
    !is_disabled_env("BERTH_E2E_K8S_ENABLED")
}

fn is_disabled_env(var: &str) -> bool {
    match std::env::var(var) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "no" | "off"
        ),
        Err(_) => false,
    }
}

fn podman_is_available() -> bool {
    Command::new("podman")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn kubectl_is_available() -> bool {
    Command::new("kubectl")
        .arg("version")
        .arg("--client=true")
        .arg("--output=yaml")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn write_executable(path: &PathBuf, content: &str) {
    fs::write(path, content).expect("Failed to write executable test fixture");
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(path)
            .expect("Failed to stat executable test fixture")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("Failed to chmod executable test fixture");
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
    assert!(stdout.contains("no workspaces configured"));
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
        .args(["rm", "delproj"])
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
        .args(["rm", "nonexistent"])
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
fn test_config_set_path_and_command() {
    let ctx = TestContext::new();
    let workspace_path = ctx.temp_dir.join("custom-workspace");

    let output = ctx
        .berth()
        .args([
            "config",
            "set",
            "cfgpath",
            "--path",
            workspace_path.to_str().unwrap(),
            "--",
            "echo",
            "hi",
        ])
        .output()
        .expect("Failed to run config set");
    assert!(output.status.success());

    let show = ctx
        .berth()
        .args(["config", "show", "cfgpath"])
        .output()
        .expect("Failed to run config show");
    assert!(show.status.success());
    let stdout = String::from_utf8_lossy(&show.stdout);
    assert!(stdout.contains(workspace_path.to_str().unwrap()));
    assert!(stdout.contains("echo hi"));

    let output = ctx
        .berth()
        .args(["config", "set", "cfgpath", "--path", "/ignored"])
        .output()
        .expect("Failed to run config set existing");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("ambiguous"));
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
fn test_shell_init_bash() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args(["shell", "init", "bash"])
        .output()
        .expect("Failed to run shell init bash");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("_berth_auto_enter_on_start"));
    assert!(stdout.contains("_berth_detect_project"));
    assert!(stdout.contains("BERTH_PROJECT_HINT"));
    assert!(!stdout.contains("WEZTERM_USER_VAR_BERTH_PROJECT"));
    assert!(stdout.contains("BASH_VERSION"));
    assert!(stdout.contains("command berth enter"));
    // Shell shorthands (`b`, `berth` wrapper) are removed; the hook
    // script must not redefine them anymore.
    assert!(!stdout.contains("\nb() {"));
    assert!(!stdout.contains("\nberth() {"));
    assert_shell_script_parses("bash", &stdout);
}

#[test]
fn test_shell_init_zsh() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args(["shell", "init", "zsh"])
        .output()
        .expect("Failed to run shell init zsh");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("_berth_auto_enter_on_start"));
    assert!(stdout.contains("ZSH_VERSION"));
    if command_exists("zsh") {
        assert_shell_script_parses("zsh", &stdout);
    }
}

fn assert_shell_script_parses(shell: &str, script: &str) {
    let mut child = Command::new(shell)
        .arg("-n")
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to run {shell} -n: {e}"));
    child
        .stdin
        .as_mut()
        .expect("stdin should be piped")
        .write_all(script.as_bytes())
        .expect("failed to write shell script to parser");
    let output = child.wait_with_output().expect("failed to wait for parser");
    assert!(
        output.status.success(),
        "{shell} -n rejected script:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn command_exists(name: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {name} >/dev/null 2>&1"))
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[test]
fn test_shell_completions_bash() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args(["shell", "completions", "bash"])
        .output()
        .expect("Failed to run shell completions bash");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("_berth()"));
    assert!(stdout.contains("COMPREPLY"));
    assert_hidden_completion_artifacts_are_absent(&stdout);
}

#[test]
fn test_shell_completions_zsh() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args(["shell", "completions", "zsh"])
        .output()
        .expect("Failed to run shell completions zsh");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("#compdef berth"));
    assert_hidden_completion_artifacts_are_absent(&stdout);
}

#[test]
fn test_shell_completions_fish_hide_internal_surface() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args(["shell", "completions", "fish"])
        .output()
        .expect("Failed to run shell completions fish");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("complete -c berth"));
    assert_hidden_completion_artifacts_are_absent(&stdout);
}

#[test]
fn test_shell_completions_elvish_hide_internal_surface() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args(["shell", "completions", "elvish"])
        .output()
        .expect("Failed to run shell completions elvish");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_hidden_completion_artifacts_are_absent(&stdout);
}

#[test]
fn test_shell_completions_powershell_hide_internal_surface() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args(["shell", "completions", "powershell"])
        .output()
        .expect("Failed to run shell completions powershell");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_hidden_completion_artifacts_are_absent(&stdout);
}

fn assert_hidden_completion_artifacts_are_absent(script: &str) {
    for hidden in ["daemon", "reap", "agent", "version-info", "hook-run"] {
        let patterns = [
            format!("-a \"{hidden}\""),
            format!("'{hidden}:"),
            format!("__fish_berth_using_subcommand {hidden}"),
        ];
        for pattern in patterns {
            assert!(
                !script.contains(&pattern),
                "hidden completion artifact {pattern:?} leaked:\n{script}"
            );
        }
    }
    for hidden_flag in ["--resume-or-new", "--supervisor", "--session-counts"] {
        assert!(
            !script.contains(hidden_flag),
            "hidden completion flag {hidden_flag:?} leaked:\n{script}"
        );
    }
    for hidden_fish_flag in ["-l resume-or-new", "-l supervisor", "-l session-counts"] {
        assert!(
            !script.contains(hidden_fish_flag),
            "hidden completion flag {hidden_fish_flag:?} leaked:\n{script}"
        );
    }
    for hidden_enter_flag in [
        "__fish_berth_using_subcommand enter\" -l new",
        "'--new[Compatibility no-op",
    ] {
        assert!(
            !script.contains(hidden_enter_flag),
            "hidden enter flag {hidden_enter_flag:?} leaked:\n{script}"
        );
    }
    if let Some(enter_case) = script.split("berth__subcmd__enter)").nth(1) {
        let enter_case = enter_case.split(";;").next().unwrap_or(enter_case);
        assert!(
            !enter_case.contains("--new"),
            "hidden enter flag leaked in bash enter completion:\n{enter_case}"
        );
    }
}

#[test]
fn test_old_shell_aliases_are_gone() {
    // No backwards-compat aliases; the canonical form is `berth shell init`
    // / `berth shell completions`. clap should reject the old forms.
    let ctx = TestContext::new();
    for args in [
        vec!["init-shell"],
        vec!["shell-init"],
        vec!["shell-completions"],
    ] {
        let output = ctx.berth().args(&args).output().expect("Failed to run");
        assert!(
            !output.status.success(),
            "expected {args:?} to fail as an unknown subcommand"
        );
    }
}

#[test]
fn test_attach_list_empty_workspace_succeeds() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args(["attach", "--list", "noproj"])
        .output()
        .expect("Failed to run attach --list");

    assert!(
        output.status.success(),
        "attach --list should succeed on empty: {:?}",
        output
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("no active sessions"));
    assert!(stdout.contains("--all --long"));
}

#[test]
fn test_attach_resume_with_no_sessions_errors_actionably() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args(["attach", "ghost"])
        .output()
        .expect("Failed to run attach");

    assert!(
        !output.status.success(),
        "attach should fail with no sessions"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no resumable session") && stderr.contains("berth enter"),
        "error should hint at how to start a session: {stderr}"
    );
}

#[test]
fn test_attach_rejects_invalid_session_id() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args(["attach", "--session", "../etc", "proj"])
        .output()
        .expect("Failed to run attach");

    assert!(
        !output.status.success(),
        "should reject path-traversal session id"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("session id"), "stderr: {stderr}");
}

#[test]
fn test_enter_rejects_hostile_workspace_name() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args(["enter", "foo;rm -rf /"])
        .output()
        .expect("Failed to run enter");

    assert!(
        !output.status.success(),
        "validator must reject shell metas"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Invalid workspace name"),
        "stderr: {stderr}"
    );
}

#[test]
fn test_help_mentions_shell_init_eval() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args(["--help"])
        .output()
        .expect("Failed to run berth --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("eval \"$(berth shell init)\""));
}

#[test]
fn test_unknown_subcommand_fails_cleanly() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args(["newproj"])
        .output()
        .expect("Failed to run berth newproj");

    // No implicit-workspace-shorthand magic anymore: clap should reject
    // an unrecognized verb outright. We don't pin the exact phrasing
    // (clap can adjust it across versions), just confirm the error
    // shape — non-zero exit and a clap-style usage hint.
    assert!(!output.status.success(), "Unknown subcommand should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unrecognized") || stderr.contains("unknown") || stderr.contains("Usage:"),
        "expected clap-style error, got: {stderr}"
    );

    let project_path = ctx.project_path("newproj");
    assert!(
        !project_path.exists(),
        "Unknown verb should not create a project"
    );
}

#[test]
fn test_enter_command_creates_and_enters() {
    let ctx = TestContext::new();
    let project_path = ctx.project_path("newproj");

    assert!(!project_path.exists(), "Project should not exist yet");

    let list_before = ctx.berth().args(["list"]).output().expect("Failed to list");
    assert!(!String::from_utf8_lossy(&list_before.stdout).contains("newproj"));

    let output = ctx
        .berth()
        .args(["enter", "newproj"])
        .output()
        .expect("Failed to run berth enter newproj");

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
fn test_enter_command_enters_existing() {
    let ctx = TestContext::new();
    let project_path = ctx.project_path("existingproj");

    ctx.berth()
        .args(["new", "existingproj", project_path.to_str().unwrap()])
        .output()
        .expect("Failed to create workspace");

    let output = ctx
        .berth()
        .args(["enter", "existingproj"])
        .output()
        .expect("Failed to run berth enter existingproj");

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
fn test_enter_command_with_remote() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args(["enter", "remotedefault", "--remote", "user@remotehost"])
        .output()
        .expect("Failed to run berth enter with remote");

    assert!(output.status.success());

    let config_path = ctx.config_dir.join("config.yaml");
    let content = fs::read_to_string(&config_path).expect("Failed to read config");
    assert!(content.contains("remotedefault"));
    assert!(content.contains("user@remotehost"));
}

#[test]
fn test_enter_remote_prints_resumable_session_command_in_skip_mode() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args(["enter", "remote-session", "--remote", "user@remotehost"])
        .output()
        .expect("Failed to run berth enter with remote");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Would SSH to user@remotehost"));
    // Each invocation gets a unique tmux/screen session id so multiple
    // local tabs don't pile into the same multiplexer session. The prefix
    // is shell-quoted; $$ and $RANDOM are unquoted so the remote shell
    // expands them at runtime.
    assert!(stdout.contains("tmux new-session -s 'berth-remote-session'-$$-$RANDOM"));
    assert!(stdout.contains("screen -S 'berth-remote-session'-$$-$RANDOM"));
    assert!(!stdout.contains("new-session -A"));
    assert!(!stdout.contains("screen -D -RR"));
    assert!(stdout.contains("else exec ${SHELL:-/bin/sh}; fi"));
}

#[test]
fn test_enter_remote_plain_flag_prints_note() {
    let ctx = TestContext::new();
    let output = ctx
        .berth()
        .args(["enter", "plainws", "--remote", "user@remotehost", "--plain"])
        .output()
        .expect("Failed to run berth enter --plain");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--plain set"),
        "expected --plain note on stderr: {stderr}"
    );
}

#[test]
fn test_enter_remote_no_deploy_flag_skips_probe_silently() {
    let ctx = TestContext::new();
    let output = ctx
        .berth()
        .args([
            "enter",
            "nodeployws",
            "--remote",
            "user@remotehost",
            "--no-deploy",
        ])
        .output()
        .expect("Failed to run berth enter --no-deploy");

    assert!(output.status.success());
    // SSH cascade still runs (skip mode prints "Would SSH ..." to stdout).
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Would SSH to user@remotehost"));
}

#[test]
fn test_enter_flags_are_mutually_exclusive() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args(["enter", "ws", "--remote", "h", "--plain", "--auto-deploy"])
        .output()
        .expect("Failed to run berth enter with conflicting flags");

    assert!(!output.status.success(), "conflicting flags should fail");
}

#[test]
fn test_deploy_subcommand_help_describes_consent_and_install_path() {
    let ctx = TestContext::new();
    let output = ctx
        .berth()
        .args(["deploy", "--help"])
        .output()
        .expect("Failed to run berth deploy --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("trusted_hosts"));
    assert!(stdout.contains("~/.local/bin/berth"));
}

#[test]
fn test_version_flag_works() {
    let ctx = TestContext::new();
    let output = ctx
        .berth()
        .args(["--version"])
        .output()
        .expect("Failed to run berth --version");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("berth "));
}

#[test]
fn test_enter_command_with_ports() {
    let ctx = TestContext::new();

    let output = ctx
        .berth()
        .args(["enter", "portdefault", "--ports", "3000,8080,9000"])
        .output()
        .expect("Failed to run berth enter with ports");

    assert!(output.status.success());

    let config_path = ctx.config_dir.join("config.yaml");
    let content = fs::read_to_string(&config_path).expect("Failed to read config");
    assert!(content.contains("portdefault"));
    assert!(content.contains("3000"));
    assert!(content.contains("8080"));
    assert!(content.contains("9000"));
}

#[test]
fn test_enter_command_recreates_missing_path() {
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
        .args(["enter", "recreateproj"])
        .output()
        .expect("Failed to run berth enter recreateproj");

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
        .args(["rm", "workflow"])
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
    assert!(stdout.contains("no workspaces configured") || stdout.contains("NAME"));
}

#[test]
fn test_podman_workspace_enter_uses_project_and_config_mounts() {
    let ctx = TestContext::new();
    let project_path = ctx.project_path("podproj");
    fs::create_dir_all(&project_path).expect("Failed to create project dir");
    let config_source = ctx.temp_dir.join("gitconfig");
    fs::write(&config_source, "[user]\n").expect("Failed to write fake config");
    let exec_log = ctx.temp_dir.join("exec.log");

    let config = format!(
        r#"defaults:
  runtime:
    type: podman
    binary: podman
    image: docker.io/library/alpine:latest
    project_mount: /workspace
workspaces:
  podproj:
    path: {}
    mounts:
      - source: {}
        target: /home/dev/.gitconfig
"#,
        project_path.display(),
        config_source.display()
    );
    fs::write(ctx.config_dir.join("config.yaml"), config).expect("Failed to write config");

    let output = ctx
        .berth_with_fake_exec(&exec_log)
        .args(["enter", "podproj"])
        .env("SHELL", "/bin/sh")
        .output()
        .expect("Failed to run berth enter");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(exec_log).expect("Missing fake exec log");
    assert!(log.contains("podman\trun"));
    assert!(log.contains("--userns=keep-id"));
    assert!(log.contains(&format!("{}:/workspace:rw", project_path.display())));
    assert!(log.contains(&format!(
        "{}:/home/dev/.gitconfig:ro",
        config_source.display()
    )));
    assert!(log.contains("docker.io/library/alpine:latest\t<command-redacted>"));
}

#[test]
fn test_podman_workspace_run_uses_container_runtime() {
    let ctx = TestContext::new();
    let project_path = ctx.project_path("podrun");
    fs::create_dir_all(&project_path).expect("Failed to create project dir");
    let exec_log = ctx.temp_dir.join("run-exec.log");

    let config = format!(
        r#"workspaces:
  podrun:
    path: {}
    runtime:
      type: podman
      image: docker.io/library/alpine:latest
"#,
        project_path.display()
    );
    fs::write(ctx.config_dir.join("config.yaml"), config).expect("Failed to write config");

    let output = ctx
        .berth_with_fake_exec(&exec_log)
        .args(["run", "podrun", "echo", "ok"])
        .output()
        .expect("Failed to run berth run");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(exec_log).expect("Missing fake exec log");
    assert!(log.contains("podman\trun"));
    assert!(log.contains("--userns=keep-id"));
    assert!(log.contains(&format!("{}:/workspace:rw", project_path.display())));
    assert!(log.contains("docker.io/library/alpine:latest\t<command-redacted>"));
}

#[test]
fn test_workspace_explicit_bare_runtime_overrides_podman_default() {
    let ctx = TestContext::new();
    let project_path = ctx.project_path("bareoverride");
    fs::create_dir_all(&project_path).expect("Failed to create project dir");

    let config = format!(
        r#"defaults:
  runtime:
    type: podman
    binary: "false"
    image: docker.io/library/alpine:latest
workspaces:
  bareoverride:
    path: {}
    runtime:
      type: bare
"#,
        project_path.display()
    );
    fs::write(ctx.config_dir.join("config.yaml"), config).expect("Failed to write config");

    let output = ctx
        .berth()
        .args(["run", "bareoverride", "sh", "-c", "pwd"])
        .output()
        .expect("Failed to run berth run");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(&project_path.display().to_string()));
}

#[test]
fn test_auto_discovery_defaults_local_workspace_to_podman() {
    let ctx = TestContext::new();
    let fake_bin = ctx.temp_dir.join("bin");
    fs::create_dir_all(&fake_bin).expect("Failed to create fake bin dir");
    write_executable(
        &fake_bin.join("podman"),
        "#!/bin/sh\nif [ \"$1\" = info ]; then printf 'true\\n'; exit 0; fi\nexit 0\n",
    );

    let project_path = ctx.project_path("autopod");
    fs::create_dir_all(&project_path).expect("Failed to create project dir");
    fs::write(
        ctx.config_dir.join("config.yaml"),
        format!(
            "workspaces:\n  autopod:\n    path: {}\n",
            project_path.display()
        ),
    )
    .expect("Failed to write config");
    let exec_log = ctx.temp_dir.join("auto-podman-exec.log");

    let output = ctx
        .berth_with_auto_discovery(&fake_bin)
        .env("BERTH_FAKE_EXEC_LOG", &exec_log)
        .args(["run", "autopod", "echo", "ok"])
        .output()
        .expect("Failed to run berth run");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(exec_log).expect("Missing fake exec log");
    assert!(log.contains("podman\trun"));
    assert!(log.contains("--userns=keep-id"));
    assert!(log.contains(&format!("{}:/workspace:rw", project_path.display())));
}

#[test]
fn test_config_bare_default_opts_out_of_auto_podman() {
    let ctx = TestContext::new();
    let fake_bin = ctx.temp_dir.join("bin");
    fs::create_dir_all(&fake_bin).expect("Failed to create fake bin dir");
    write_executable(
        &fake_bin.join("podman"),
        "#!/bin/sh\nif [ \"$1\" = info ]; then printf 'true\\n'; exit 0; fi\nexit 42\n",
    );

    let project_path = ctx.project_path("bareauto");
    fs::create_dir_all(&project_path).expect("Failed to create project dir");
    fs::write(
        ctx.config_dir.join("config.yaml"),
        format!(
            "defaults:\n  runtime:\n    type: bare\nworkspaces:\n  bareauto:\n    path: {}\n",
            project_path.display()
        ),
    )
    .expect("Failed to write config");

    let output = ctx
        .berth_with_auto_discovery(&fake_bin)
        .args(["run", "bareauto", "sh", "-c", "pwd"])
        .output()
        .expect("Failed to run berth run");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(&project_path.display().to_string()));
}

#[test]
fn test_doctor_reports_podman_and_minikube_discovery() {
    let ctx = TestContext::new();
    let fake_bin = ctx.temp_dir.join("bin");
    fs::create_dir_all(&fake_bin).expect("Failed to create fake bin dir");
    write_executable(
        &fake_bin.join("podman"),
        "#!/bin/sh\nif [ \"$1\" = info ]; then printf 'true\\n'; exit 0; fi\nexit 0\n",
    );
    write_executable(&fake_bin.join("kubectl"), "#!/bin/sh\nexit 0\n");
    write_executable(
        &fake_bin.join("minikube"),
        r#"#!/bin/sh
if [ "$1" = profile ] && [ "$2" = list ]; then
  printf '{"valid":[{"Name":"minikube","Config":{"Driver":"podman","Rootless":true}}]}'
  exit 0
fi
exit 1
"#,
    );

    let output = ctx
        .berth_with_auto_discovery(&fake_bin)
        .args(["doctor"])
        .output()
        .expect("Failed to run berth doctor");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Auto-discovery: enabled"));
    assert!(stdout.contains("Default local runtime: podman"));
    assert!(stdout.contains("Podman: ready"));
    assert!(stdout.contains("minikube: ready"));
    assert!(stdout.contains("Kubernetes pod defaults: available namespace=berth"));
}

#[test]
fn test_reap_stops_only_expired_local_podman_containers() {
    let ctx = TestContext::new();
    let expired_path = ctx.project_path("podidle");
    let active_path = ctx.project_path("podactive");
    let bare_path = ctx.project_path("bareidle");
    let remote_path = ctx.project_path("remoteidle");
    fs::create_dir_all(&expired_path).expect("Failed to create expired project dir");
    fs::create_dir_all(&active_path).expect("Failed to create active project dir");
    fs::create_dir_all(&bare_path).expect("Failed to create bare project dir");
    fs::create_dir_all(&remote_path).expect("Failed to create remote project dir");
    let exec_log = ctx.temp_dir.join("reap-exec.log");

    let config = format!(
        r#"workspaces:
  podidle:
    path: {}
    runtime:
      type: podman
      image: docker.io/library/alpine:latest
  podactive:
    path: {}
    runtime:
      type: podman
      image: docker.io/library/alpine:latest
  bareidle:
    path: {}
  remoteidle:
    path: {}
    remote: white-vm2
    runtime:
      type: podman
      image: docker.io/library/alpine:latest
"#,
        expired_path.display(),
        active_path.display(),
        bare_path.display(),
        remote_path.display()
    );
    fs::write(ctx.config_dir.join("config.yaml"), config).expect("Failed to write config");

    let state_dir = ctx.data_dir().join("berth");
    fs::create_dir_all(&state_dir).expect("Failed to create state dir");
    fs::write(
        state_dir.join("lifecycle.json"),
        r#"{
  "environments": {
    "podidle": {
      "workspace": "podidle",
      "host": null,
      "runtime": "podman",
      "last_active_epoch_seconds": 1,
      "idle_shutdown_after_seconds": 1
    },
    "podactive": {
      "workspace": "podactive",
      "host": null,
      "runtime": "podman",
      "last_active_epoch_seconds": 4102444800,
      "idle_shutdown_after_seconds": 31536000
    },
    "bareidle": {
      "workspace": "bareidle",
      "host": null,
      "runtime": "bare",
      "last_active_epoch_seconds": 1,
      "idle_shutdown_after_seconds": 1
    },
    "remoteidle@white-vm2": {
      "workspace": "remoteidle",
      "host": "white-vm2",
      "runtime": "podman",
      "last_active_epoch_seconds": 1,
      "idle_shutdown_after_seconds": 1
    }
  }
}
"#,
    )
    .expect("Failed to write lifecycle state");

    let output = ctx
        .berth_with_fake_exec(&exec_log)
        .args(["reap"])
        .output()
        .expect("Failed to run berth reap");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Stopped expired container 'berth-podidle'"));
    assert!(stdout.contains("Reaped 1 environment(s)"));

    let log = fs::read_to_string(exec_log).expect("Missing fake exec log");
    assert!(log.contains("podman\tstop\tberth-podidle"));
    assert!(!log.contains("berth-podactive"));
    assert!(!log.contains("berth-bareidle"));
    assert!(!log.contains("berth-remoteidle"));

    let lifecycle =
        fs::read_to_string(state_dir.join("lifecycle.json")).expect("Missing lifecycle state");
    assert!(!lifecycle.contains("\"podidle\""));
    assert!(lifecycle.contains("\"podactive\""));
    assert!(lifecycle.contains("\"bareidle\""));
    assert!(lifecycle.contains("\"remoteidle@white-vm2\""));
}

#[test]
fn test_daemon_once_runs_idle_reaper() {
    let ctx = TestContext::new();
    let project_path = ctx.project_path("daemonidle");
    fs::create_dir_all(&project_path).expect("Failed to create project dir");
    let exec_log = ctx.temp_dir.join("daemon-exec.log");

    let config = format!(
        r#"workspaces:
  daemonidle:
    path: {}
    runtime:
      type: podman
      image: docker.io/library/alpine:latest
"#,
        project_path.display(),
    );
    fs::write(ctx.config_dir.join("config.yaml"), config).expect("Failed to write config");

    let state_dir = ctx.data_dir().join("berth");
    fs::create_dir_all(&state_dir).expect("Failed to create state dir");
    fs::write(
        state_dir.join("lifecycle.json"),
        r#"{
  "environments": {
    "daemonidle": {
      "workspace": "daemonidle",
      "host": null,
      "runtime": "podman",
      "last_active_epoch_seconds": 1,
      "idle_shutdown_after_seconds": 1
    }
  }
}
"#,
    )
    .expect("Failed to write lifecycle state");

    let output = ctx
        .berth_with_fake_exec(&exec_log)
        .args(["daemon", "--interval-seconds", "1", "--once"])
        .output()
        .expect("Failed to run berth daemon");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Berth daemon running in foreground"));
    assert!(stdout.contains("Stopped expired container 'berth-daemonidle'"));
    assert!(stdout.contains("Berth daemon one-shot run complete"));

    let log = fs::read_to_string(exec_log).expect("Missing fake exec log");
    assert!(log.contains("podman\tstop\tberth-daemonidle"));

    let lifecycle =
        fs::read_to_string(state_dir.join("lifecycle.json")).expect("Missing lifecycle state");
    assert!(!lifecycle.contains("\"daemonidle\""));
}

#[test]
fn test_kubernetes_pod_workspace_run_constructs_kubectl_run() {
    let ctx = TestContext::new();
    let project_path = ctx.project_path("kubepod");
    fs::create_dir_all(&project_path).expect("Failed to create project dir");
    let exec_log = ctx.temp_dir.join("kubectl-run.log");

    let config = format!(
        r#"workspaces:
  kubepod:
    path: {}
    runtime:
      type: kubernetes-pod
      kubectl: kubectl
      image: docker.io/library/alpine:latest
      namespace: dev
      pod_name: berth-custom
"#,
        project_path.display()
    );
    fs::write(ctx.config_dir.join("config.yaml"), config).expect("Failed to write config");

    let output = ctx
        .berth_with_fake_exec(&exec_log)
        .args(["run", "kubepod", "echo", "ok"])
        .output()
        .expect("Failed to run berth run");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(exec_log).expect("Missing fake exec log");
    assert!(log.contains("kubectl\trun\tberth-custom"));
    assert!(log.contains("--namespace\tdev"));
    assert!(log.contains("--image\tdocker.io/library/alpine:latest"));
    assert!(log.contains("--restart\tNever"));
    assert!(log.contains("--attach"));
    assert!(log.contains("--rm"));
    assert!(log.contains("--command\t--\t<command-redacted>"));
}

#[test]
fn test_reap_expired_kubernetes_pod_deletes_pod_and_updates_state() {
    let ctx = TestContext::new();
    let project_path = ctx.project_path("oldpod");
    fs::create_dir_all(&project_path).expect("Failed to create project dir");
    let exec_log = ctx.temp_dir.join("kubectl-reap.log");

    let config = format!(
        r#"workspaces:
  oldpod:
    path: {}
    runtime:
      type: kubernetes-pod
      kubectl: kubectl
      image: docker.io/library/alpine:latest
      namespace: dev
      pod_name: berth-oldpod
    idle:
      shutdown_after_seconds: 1
"#,
        project_path.display()
    );
    fs::write(ctx.config_dir.join("config.yaml"), config).expect("Failed to write config");

    let state_dir = ctx.data_dir().join("berth");
    fs::create_dir_all(&state_dir).expect("Failed to create state dir");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_secs();
    let old = now.saturating_sub(60);
    let state = format!(
        r#"{{
  "environments": {{
    "oldpod": {{
      "workspace": "oldpod",
      "host": null,
      "runtime": "kubernetes-pod",
      "last_active_epoch_seconds": {},
      "idle_shutdown_after_seconds": 1
    }}
  }}
}}"#,
        old
    );
    fs::write(state_dir.join("lifecycle.json"), state).expect("Failed to write lifecycle state");

    let output = ctx
        .berth_with_fake_exec(&exec_log)
        .args(["reap"])
        .output()
        .expect("Failed to run berth reap");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(exec_log).expect("Missing fake exec log");
    assert!(log.contains("kubectl\tdelete\tpod\tberth-oldpod"));
    assert!(log.contains("--namespace\tdev"));
    assert!(log.contains("--ignore-not-found=true"));

    let updated_state =
        fs::read_to_string(state_dir.join("lifecycle.json")).expect("Failed to read lifecycle");
    assert!(!updated_state.contains("\"oldpod\""));
}

#[test]
fn test_stop_kubernetes_pod_workspace_deletes_configured_pod() {
    let ctx = TestContext::new();
    let project_path = ctx.project_path("stoppod");
    fs::create_dir_all(&project_path).expect("Failed to create project dir");
    let exec_log = ctx.temp_dir.join("kubectl-stop.log");

    let config = format!(
        r#"workspaces:
  stoppod:
    path: {}
    runtime:
      type: kubernetes-pod
      kubectl: kubectl
      image: docker.io/library/alpine:latest
      namespace: dev
      pod_name: berth-stoppod
"#,
        project_path.display()
    );
    fs::write(ctx.config_dir.join("config.yaml"), config).expect("Failed to write config");

    let output = ctx
        .berth_with_fake_exec(&exec_log)
        .args(["stop", "stoppod"])
        .output()
        .expect("Failed to run berth stop");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fs::read_to_string(exec_log).expect("Missing fake exec log");
    assert!(log.contains("kubectl\tdelete\tpod\tberth-stoppod"));
    assert!(log.contains("--namespace\tdev"));
    assert!(log.contains("--ignore-not-found=true"));
}

#[test]
fn test_real_podman_workspace_run_executes_in_container() {
    if !real_podman_e2e_enabled() {
        eprintln!("skipping real podman e2e (BERTH_E2E_PODMAN_ENABLED disables it)");
        return;
    }
    assert!(
        podman_is_available(),
        "podman e2e is enabled by default; install podman or set BERTH_E2E_PODMAN_ENABLED=false"
    );

    let ctx = TestContext::new();
    let project_path = ctx.project_path("realpod");
    fs::create_dir_all(&project_path).expect("Failed to create project dir");
    fs::write(project_path.join("input.txt"), "project-ok\n").expect("Failed to write input");

    let config_source = ctx.temp_dir.join("config-mount");
    fs::create_dir_all(&config_source).expect("Failed to create config mount dir");
    fs::write(config_source.join("message.txt"), "config-ok\n")
        .expect("Failed to write config mount file");

    let config = format!(
        r#"workspaces:
  realpod:
    path: {}
    runtime:
      type: podman
      image: docker.io/library/alpine:latest
    mounts:
      - source: {}
        target: /mnt/berth-config
"#,
        project_path.display(),
        config_source.display()
    );
    fs::write(ctx.config_dir.join("config.yaml"), config).expect("Failed to write config");

    let script = "test \"$(cat input.txt)\" = project-ok && \
                  test \"$(cat /mnt/berth-config/message.txt)\" = config-ok && \
                  printf container-ok > generated.txt && \
                  cat generated.txt";
    let output = ctx
        .berth_with_host_container_runtime()
        .args(["run", "realpod", "sh", "-c", script])
        .output()
        .expect("Failed to run berth real podman e2e");

    assert!(
        output.status.success(),
        "Command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("container-ok"),
        "Container stdout should include command output"
    );
    assert_eq!(
        fs::read_to_string(project_path.join("generated.txt"))
            .expect("Container should write into project mount"),
        "container-ok"
    );
}

#[test]
fn test_real_podman_daemon_once_reaps_live_container() {
    if !real_podman_e2e_enabled() {
        eprintln!("skipping real podman daemon e2e (BERTH_E2E_PODMAN_ENABLED disables it)");
        return;
    }
    assert!(
        podman_is_available(),
        "podman e2e is enabled by default; install podman or set BERTH_E2E_PODMAN_ENABLED=false"
    );

    let ctx = TestContext::new();
    let workspace = format!("daemonreal{}", std::process::id());
    let container = format!("berth-{}", workspace);
    let project_path = ctx.project_path(&workspace);
    fs::create_dir_all(&project_path).expect("Failed to create project dir");

    let _ = Command::new("podman")
        .args(["rm", "-f", &container])
        .status();
    let started = Command::new("podman")
        .args([
            "run",
            "-d",
            "--name",
            &container,
            "docker.io/library/alpine:latest",
            "sleep",
            "300",
        ])
        .output()
        .expect("Failed to start podman test container");
    assert!(
        started.status.success(),
        "Failed to start test container\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&started.stdout),
        String::from_utf8_lossy(&started.stderr)
    );

    let config = format!(
        r#"workspaces:
  {}:
    path: {}
    runtime:
      type: podman
      image: docker.io/library/alpine:latest
"#,
        workspace,
        project_path.display(),
    );
    fs::write(ctx.config_dir.join("config.yaml"), config).expect("Failed to write config");

    let state_dir = ctx.data_dir().join("berth");
    fs::create_dir_all(&state_dir).expect("Failed to create state dir");
    let state = format!(
        r#"{{
  "environments": {{
    "{}": {{
      "workspace": "{}",
      "host": null,
      "runtime": "podman",
      "last_active_epoch_seconds": 1,
      "idle_shutdown_after_seconds": 1
    }}
  }}
}}"#,
        workspace, workspace
    );
    fs::write(state_dir.join("lifecycle.json"), state).expect("Failed to write lifecycle state");

    let output = ctx
        .berth_with_host_container_runtime()
        .args(["daemon", "--interval-seconds", "1", "--once"])
        .output()
        .expect("Failed to run berth daemon");

    let inspect = Command::new("podman")
        .args(["inspect", "-f", "{{.State.Status}}", &container])
        .output()
        .expect("Failed to inspect podman test container");
    let _ = Command::new("podman")
        .args(["rm", "-f", &container])
        .status();

    assert!(
        output.status.success(),
        "Command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        inspect.status.success(),
        "Container should still be inspectable before cleanup\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&inspect.stdout),
        String::from_utf8_lossy(&inspect.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&inspect.stdout).trim(), "exited");
}

#[test]
fn test_real_k8s_workspace_run_executes_in_pod() {
    if !real_k8s_e2e_enabled() {
        eprintln!("skipping real k8s e2e (BERTH_E2E_K8S_ENABLED disables it)");
        return;
    }
    assert!(
        kubectl_is_available(),
        "k8s e2e is enabled by default; install kubectl + a reachable cluster, \
         or set BERTH_E2E_K8S_ENABLED=false in .env"
    );

    let ctx = TestContext::new();
    let workspace = format!("k8sreal{}", std::process::id());
    let project_path = ctx.project_path(&workspace);
    fs::create_dir_all(&project_path).expect("Failed to create project dir");

    let namespace = std::env::var("BERTH_REAL_K8S_NAMESPACE").unwrap_or_else(|_| "berth".into());
    let kubectl = std::env::var("BERTH_REAL_K8S_KUBECTL").unwrap_or_else(|_| "kubectl".into());
    let image = std::env::var("BERTH_REAL_K8S_IMAGE")
        .unwrap_or_else(|_| "docker.io/library/alpine:3.20".into());

    let _ = Command::new(&kubectl)
        .args(["create", "namespace", &namespace])
        .output();

    let config = format!(
        r#"workspaces:
  {workspace}:
    path: {path}
    runtime:
      type: kubernetes-pod
      image: {image}
      namespace: {namespace}
      kubectl: {kubectl}
"#,
        workspace = workspace,
        path = project_path.display(),
        image = image,
        namespace = namespace,
        kubectl = kubectl,
    );
    fs::write(ctx.config_dir.join("config.yaml"), config).expect("Failed to write config");

    let output = ctx
        .berth_with_host_container_runtime()
        .args(["run", &workspace, "echo", "k8s-pod-ok"])
        .output()
        .expect("Failed to run berth real k8s e2e");

    let pod_name = format!("berth-{}", workspace);
    let _ = Command::new(&kubectl)
        .args([
            "delete",
            "pod",
            &pod_name,
            "--namespace",
            &namespace,
            "--ignore-not-found=true",
        ])
        .status();

    assert!(
        output.status.success(),
        "Command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("k8s-pod-ok"),
        "Pod stdout should include command output, got:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
