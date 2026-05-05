use std::path::PathBuf;

use super::{
    validate_bind_mount, validate_command, BindMount, CommandSpec, ConfiguredMount, ProjectMount,
    RuntimeCommandError,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodmanRunConfig {
    pub image: String,
    pub project: ProjectMount,
    pub mounts: Vec<ConfiguredMount>,
    pub command: Vec<String>,
    pub remove: bool,
    pub interactive: bool,
    pub tty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodmanStopConfig {
    pub container: String,
}

impl PodmanStopConfig {
    pub fn new(container: impl Into<String>) -> Self {
        Self {
            container: container.into(),
        }
    }
}

impl PodmanRunConfig {
    pub fn new(
        image: impl Into<String>,
        project_dir: impl Into<PathBuf>,
        command: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            image: image.into(),
            project: ProjectMount::new(project_dir),
            mounts: Vec::new(),
            command: command.into_iter().map(Into::into).collect(),
            remove: true,
            interactive: true,
            tty: true,
        }
    }

    pub fn with_project(mut self, project: ProjectMount) -> Self {
        self.project = project;
        self
    }

    pub fn with_mounts(mut self, mounts: impl IntoIterator<Item = ConfiguredMount>) -> Self {
        self.mounts = mounts.into_iter().collect();
        self
    }

    pub fn with_tty(mut self, tty: bool) -> Self {
        self.tty = tty;
        self
    }

    pub fn with_interactive(mut self, interactive: bool) -> Self {
        self.interactive = interactive;
        self
    }
}

pub fn build_command(config: &PodmanRunConfig) -> Result<CommandSpec, RuntimeCommandError> {
    if config.image.trim().is_empty() {
        return Err(RuntimeCommandError::EmptyImage);
    }
    validate_command(&config.command)?;

    let mounts = bind_mounts(config);
    for mount in &mounts {
        validate_bind_mount(mount)?;
    }

    let mut args = Vec::new();
    args.push("run".to_string());
    if config.remove {
        args.push("--rm".to_string());
    }
    if config.interactive {
        args.push("--interactive".to_string());
    }
    if config.tty {
        args.push("--tty".to_string());
    }
    args.push("--workdir".to_string());
    args.push(config.project.target.display().to_string());

    for mount in mounts {
        args.push("--volume".to_string());
        args.push(mount.podman_volume_arg());
    }

    args.push(config.image.clone());
    args.extend(config.command.clone());

    Ok(CommandSpec::new("podman").with_args(args))
}

pub fn build_stop_command(config: &PodmanStopConfig) -> Result<CommandSpec, RuntimeCommandError> {
    if config.container.trim().is_empty() {
        return Err(RuntimeCommandError::EmptyCommand);
    }

    Ok(CommandSpec::new("podman").with_args(["stop", config.container.as_str()]))
}

pub fn bind_mounts(config: &PodmanRunConfig) -> Vec<BindMount> {
    let mut mounts = Vec::with_capacity(config.mounts.len() + 1);
    mounts.push(config.project.as_bind_mount());
    mounts.extend(config.mounts.iter().map(ConfiguredMount::as_bind_mount));
    mounts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{MountAccess, RuntimeCommandError};

    #[test]
    fn builds_podman_run_with_project_and_configured_mounts() {
        let config = PodmanRunConfig::new("rust:latest", "/src/project", ["cargo", "test"])
            .with_mounts([
                ConfiguredMount::new("/host/cargo", "/cargo"),
                ConfiguredMount::read_write("/host/cache", "/cache"),
            ])
            .with_tty(false);

        let command = build_command(&config).expect("command should build");

        assert_eq!(command.program, "podman");
        assert_eq!(
            command.args,
            [
                "run",
                "--rm",
                "--interactive",
                "--workdir",
                "/workspace",
                "--volume",
                "/src/project:/workspace:rw",
                "--volume",
                "/host/cargo:/cargo:ro",
                "--volume",
                "/host/cache:/cache:rw",
                "rust:latest",
                "cargo",
                "test"
            ]
        );
    }

    #[test]
    fn validates_configured_mount_targets() {
        let config = PodmanRunConfig::new("alpine", "/src/project", ["true"])
            .with_mounts([ConfiguredMount::new("/host/cache", "relative")]);

        assert_eq!(
            build_command(&config),
            Err(RuntimeCommandError::RelativeMountTarget(PathBuf::from(
                "relative"
            )))
        );
    }

    #[test]
    fn supports_read_only_project_mounts() {
        let project = ProjectMount::new("/src/project").with_access(MountAccess::ReadOnly);
        let config = PodmanRunConfig::new("alpine", "/unused", ["true"]).with_project(project);

        let mounts = bind_mounts(&config);

        assert_eq!(mounts[0].podman_volume_arg(), "/src/project:/workspace:ro");
    }

    #[test]
    fn builds_podman_stop_command() {
        let command = build_stop_command(&PodmanStopConfig::new("berth-project")).unwrap();

        assert_eq!(command.program, "podman");
        assert_eq!(command.args, ["stop", "berth-project"]);
    }
}
