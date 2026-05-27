pub mod bare;
pub mod kubernetes;
pub mod podman;

use std::collections::BTreeMap;
#[cfg(debug_assertions)]
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
}

impl CommandSpec {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
        }
    }

    pub fn with_args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    pub fn argv(&self) -> Vec<String> {
        let mut argv = Vec::with_capacity(self.args.len() + 1);
        argv.push(self.program.clone());
        argv.extend(self.args.clone());
        argv
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeKind {
    Bare,
    KubernetesPod,
    Podman,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountAccess {
    ReadOnly,
    ReadWrite,
}

impl MountAccess {
    pub fn podman_suffix(self) -> &'static str {
        match self {
            Self::ReadOnly => "ro",
            Self::ReadWrite => "rw",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindMount {
    pub source: PathBuf,
    pub target: PathBuf,
    pub access: MountAccess,
}

impl BindMount {
    pub fn read_only(source: impl Into<PathBuf>, target: impl Into<PathBuf>) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            access: MountAccess::ReadOnly,
        }
    }

    pub fn read_write(source: impl Into<PathBuf>, target: impl Into<PathBuf>) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            access: MountAccess::ReadWrite,
        }
    }

    pub fn podman_volume_arg(&self) -> String {
        format!(
            "{}:{}:{}",
            self.source.display(),
            self.target.display(),
            self.access.podman_suffix()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMount {
    pub source: PathBuf,
    pub target: PathBuf,
    pub access: MountAccess,
}

impl ProjectMount {
    pub fn new(source: impl Into<PathBuf>) -> Self {
        Self {
            source: source.into(),
            target: PathBuf::from("/workspace"),
            access: MountAccess::ReadWrite,
        }
    }

    pub fn with_target(mut self, target: impl Into<PathBuf>) -> Self {
        self.target = target.into();
        self
    }

    pub fn with_access(mut self, access: MountAccess) -> Self {
        self.access = access;
        self
    }

    pub fn as_bind_mount(&self) -> BindMount {
        BindMount {
            source: self.source.clone(),
            target: self.target.clone(),
            access: self.access,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredMount {
    pub source: PathBuf,
    pub target: PathBuf,
    pub access: MountAccess,
}

impl ConfiguredMount {
    pub fn new(source: impl Into<PathBuf>, target: impl Into<PathBuf>) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            access: MountAccess::ReadOnly,
        }
    }

    pub fn read_write(source: impl Into<PathBuf>, target: impl Into<PathBuf>) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            access: MountAccess::ReadWrite,
        }
    }

    pub fn as_bind_mount(&self) -> BindMount {
        BindMount {
            source: self.source.clone(),
            target: self.target.clone(),
            access: self.access,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeCommandError {
    #[error("command cannot be empty")]
    EmptyCommand,
    #[error("container image cannot be empty")]
    EmptyImage,
    #[error("mount target must be absolute: {0}")]
    RelativeMountTarget(PathBuf),
    #[error("mount source cannot be empty")]
    EmptyMountSource,
}

pub fn validate_bind_mount(mount: &BindMount) -> Result<(), RuntimeCommandError> {
    if mount.source.as_os_str().is_empty() {
        return Err(RuntimeCommandError::EmptyMountSource);
    }
    if !mount.target.is_absolute() {
        return Err(RuntimeCommandError::RelativeMountTarget(
            mount.target.clone(),
        ));
    }
    Ok(())
}

pub fn validate_configured_mounts(mounts: &[crate::config::Mount]) -> anyhow::Result<()> {
    for mount in mounts {
        if !Path::new(&mount.target).is_absolute() {
            anyhow::bail!("Mount target must be absolute: {}", mount.target);
        }
        if mount.required {
            let source = expand_home(&mount.source);
            if !source.exists() {
                anyhow::bail!("Required mount source does not exist: {}", source.display());
            }
        }
    }
    Ok(())
}

fn expand_home(path: &str) -> PathBuf {
    if path == "~" {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from(path))
    } else if let Some(rest) = path.strip_prefix("~/") {
        dirs::home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(path))
    } else {
        PathBuf::from(path)
    }
}

pub fn validate_command(command: &[String]) -> Result<(), RuntimeCommandError> {
    if command.is_empty() || command[0].trim().is_empty() {
        return Err(RuntimeCommandError::EmptyCommand);
    }
    Ok(())
}

pub fn path_inside(base: &Path, relative: impl AsRef<Path>) -> PathBuf {
    let relative = relative.as_ref();
    if relative.as_os_str().is_empty() {
        return base.to_path_buf();
    }
    base.join(relative)
}

pub fn run_command(spec: &CommandSpec) -> std::io::Result<std::process::ExitStatus> {
    #[cfg(debug_assertions)]
    if let Ok(log_path) = std::env::var("BERTH_FAKE_EXEC_LOG") {
        write_fake_exec_log(&log_path, spec)?;
        return std::process::Command::new("true").status();
    }

    let mut command = std::process::Command::new(&spec.program);
    command.args(&spec.args);
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    command.status()
}

pub fn output_command(spec: &CommandSpec) -> std::io::Result<std::process::Output> {
    #[cfg(debug_assertions)]
    if let Ok(log_path) = std::env::var("BERTH_FAKE_EXEC_LOG") {
        write_fake_exec_log(&log_path, spec)?;
        return std::process::Command::new("true").output();
    }

    let mut command = std::process::Command::new(&spec.program);
    command.args(&spec.args);
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    command.output()
}

#[cfg(debug_assertions)]
fn write_fake_exec_log(log_path: &str, spec: &CommandSpec) -> std::io::Result<()> {
    let mut line = fake_exec_argv(spec).join("\t");
    if spec.cwd.is_some() {
        line.push_str("\tcwd=<redacted>");
    }
    line.push('\n');
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?
        .write_all(line.as_bytes())
}

#[cfg(debug_assertions)]
fn fake_exec_argv(spec: &CommandSpec) -> Vec<String> {
    match spec.program.as_str() {
        "podman" => fake_exec_podman_argv(spec),
        "kubectl" => fake_exec_kubectl_argv(spec),
        program => {
            let mut argv = vec![program.to_string()];
            if !spec.args.is_empty() {
                argv.push("<args-redacted>".to_string());
            }
            argv
        }
    }
}

#[cfg(debug_assertions)]
fn fake_exec_podman_argv(spec: &CommandSpec) -> Vec<String> {
    let mut argv = vec![spec.program.clone()];
    let mut args = spec.args.iter();
    while let Some(arg) = args.next() {
        argv.push(redact_arg(arg));
        if sensitive_arg(arg) && !arg.contains('=') {
            if args.next().is_some() {
                argv.push("<redacted>".to_string());
            }
            continue;
        }
        if matches!(
            arg.as_str(),
            "--volume" | "-v" | "--workdir" | "--name" | "--userns"
        ) {
            if let Some(value) = args.next() {
                argv.push(redact_arg(value));
            }
            continue;
        }
        if arg == "stop" {
            if let Some(container) = args.next() {
                argv.push(redact_arg(container));
            }
            break;
        }
        if !arg.starts_with('-') && arg != "run" {
            argv.push("<command-redacted>".to_string());
            break;
        }
    }
    argv
}

#[cfg(debug_assertions)]
fn fake_exec_kubectl_argv(spec: &CommandSpec) -> Vec<String> {
    let mut argv = vec![spec.program.clone()];
    let mut args = spec.args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            argv.push("--".to_string());
            argv.push("<command-redacted>".to_string());
            break;
        }
        argv.push(redact_arg(arg));
        if sensitive_arg(arg) && !arg.contains('=') && args.next().is_some() {
            argv.push("<redacted>".to_string());
        }
    }
    argv
}

#[cfg(debug_assertions)]
fn redact_arg(arg: &str) -> String {
    if sensitive_arg(arg) {
        "<redacted>".to_string()
    } else {
        arg.to_string()
    }
}

#[cfg(debug_assertions)]
fn sensitive_arg(arg: &str) -> bool {
    let upper = arg.to_ascii_uppercase();
    upper.contains("KEY")
        || upper.contains("SECRET")
        || upper.contains("TOKEN")
        || upper.contains("PASS")
        || upper.contains("CRED")
        || upper.contains("AUTH")
        || upper.contains("COOKIE")
        || upper.contains("DATABASE_URL")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_mount_defaults_to_read_only() {
        let mount = ConfiguredMount::new("/host/cache", "/cache");

        assert_eq!(mount.access, MountAccess::ReadOnly);
        assert_eq!(
            mount.as_bind_mount().podman_volume_arg(),
            "/host/cache:/cache:ro"
        );
    }

    #[test]
    fn project_mount_defaults_to_workspace_read_write() {
        let mount = ProjectMount::new("/project");

        assert_eq!(mount.target, PathBuf::from("/workspace"));
        assert_eq!(mount.access, MountAccess::ReadWrite);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn fake_exec_log_redacts_secret_flag_values() {
        let spec = CommandSpec::new("kubectl").with_args([
            "--token",
            "supersecret",
            "run",
            "pod",
            "--",
            "echo",
            "ok",
        ]);
        let argv = fake_exec_argv(&spec);
        assert_eq!(
            argv,
            [
                "kubectl",
                "<redacted>",
                "<redacted>",
                "run",
                "pod",
                "--",
                "<command-redacted>",
            ]
        );
        assert!(!argv.iter().any(|arg| arg == "supersecret"));
    }
}
