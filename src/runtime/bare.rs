use std::path::PathBuf;

use super::{validate_command, CommandSpec, RuntimeCommandError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BareRunConfig {
    pub project_dir: PathBuf,
    pub command: Vec<String>,
}

impl BareRunConfig {
    pub fn new(
        project_dir: impl Into<PathBuf>,
        command: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            project_dir: project_dir.into(),
            command: command.into_iter().map(Into::into).collect(),
        }
    }
}

pub fn build_command(config: &BareRunConfig) -> Result<CommandSpec, RuntimeCommandError> {
    validate_command(&config.command)?;

    Ok(CommandSpec::new(&config.command[0])
        .with_args(config.command.iter().skip(1).cloned())
        .with_cwd(config.project_dir.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_bare_command_with_project_cwd() {
        let config = BareRunConfig::new("/project", ["cargo", "test", "--quiet"]);
        let command = build_command(&config).expect("command should build");

        assert_eq!(command.program, "cargo");
        assert_eq!(command.args, ["test", "--quiet"]);
        assert_eq!(command.cwd, Some(PathBuf::from("/project")));
    }

    #[test]
    fn rejects_empty_bare_command() {
        let config = BareRunConfig::new("/project", std::iter::empty::<String>());

        assert_eq!(
            build_command(&config),
            Err(RuntimeCommandError::EmptyCommand)
        );
    }
}
