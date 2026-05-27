use anyhow::{bail, Result};
use berth::config::Config;
use berth::deploy::{self, DeployDecision};
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

pub async fn run(host: String, tag: Option<String>, force: bool) -> Result<()> {
    deploy::freshness::warn_if_stale().await;

    let mut config = Config::load()?;
    let local_version = berth::build_info::version().to_string();
    let local_build = berth::build_info::build_id();
    let local_os = std::env::consts::OS;
    let local_arch = std::env::consts::ARCH;

    let probe_spinner = phase_spinner(&format!("probing {host}"));
    let env = deploy::probe(&host).await?;
    probe_spinner.finish_and_clear();

    let remote_state = env
        .berth_version
        .as_deref()
        .map(|v| format!("berth {v}"))
        .unwrap_or_else(|| "no existing berth".to_string());
    eprintln!("  local:  {local_os} / {local_arch}  ({local_version}, {local_build})");
    eprintln!("  remote: {} / {}  ({})", env.os, env.arch, remote_state);

    let decision = if tag.is_some() {
        match deploy::target_triple(&env.os, &env.arch) {
            Some(target) => DeployDecision::Deploy {
                target,
                reason: "explicit release tag requested".to_string(),
                source: deploy::DeploySource::Release,
            },
            None => DeployDecision::UnsupportedArch {
                os: env.os.clone(),
                arch: env.arch.clone(),
            },
        }
    } else {
        deploy::decide(&env, &local_version, local_build)
    };
    let target = match &decision {
        DeployDecision::UnsupportedArch { os, arch } => {
            bail!(
                "no pre-built berth for {os}/{arch}; install tmux or screen on {host} \
                 and use `berth enter --plain --remote {host} <ws>` for non-resumable shells"
            );
        }
        DeployDecision::UpToDate => {
            let Some(t) = deploy::target_triple(&env.os, &env.arch) else {
                bail!(
                    "internal: arch {}/{} reported UpToDate but no target triple — \
                     please file a bug",
                    env.os,
                    env.arch
                );
            };
            if !force {
                eprintln!(
                    "  {host} is up to date (berth {local_version}); pass --force to redeploy"
                );
                return Ok(());
            }
            t
        }
        DeployDecision::LocalBuildUnsupported {
            target,
            local_target,
            reason,
        } => {
            bail!(
                "{reason}; local binary target {:?} cannot be deployed to remote target {target}. \
                 Use a release tag built for {target} or run deploy from a matching host.",
                local_target
            );
        }
        DeployDecision::Deploy { target, .. } => target,
    };
    eprintln!("  target: {target}");

    let info = if should_deploy_local(tag.as_deref(), force, &decision, target) {
        deploy::ensure_deployed_local(&host, target).await?
    } else {
        let tag = tag.unwrap_or_else(|| format!("v{local_version}"));
        deploy::ensure_deployed(&host, &tag, target).await?
    };
    deploy::record_trust(&mut config, &host, &info)?;

    eprintln!(
        "berth: deployed v{} to {}:{}  (host added to trusted_hosts)",
        info.version,
        host,
        info.remote_path.display()
    );
    Ok(())
}

fn should_deploy_local(
    explicit_tag: Option<&str>,
    force: bool,
    decision: &DeployDecision,
    target: &str,
) -> bool {
    explicit_tag.is_none()
        && (matches!(
            decision,
            DeployDecision::Deploy {
                source: deploy::DeploySource::LocalBinary,
                ..
            }
        ) || (force && deploy::local_binary_compatible_with(target)))
}

fn phase_spinner(message: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.enable_steady_tick(Duration::from_millis(80));
    pb.set_style(
        ProgressStyle::with_template("  {msg:<28.cyan} {spinner}")
            .unwrap()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "),
    );
    pb.set_message(message.to_string());
    pb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_deploy_uses_local_binary_when_compatible() {
        let target = berth::build_info::build_target();
        assert!(should_deploy_local(
            None,
            true,
            &DeployDecision::UpToDate,
            target
        ));
    }

    #[test]
    fn explicit_tag_uses_release_even_when_force_is_set() {
        let target = berth::build_info::build_target();
        assert!(!should_deploy_local(
            Some("v0.1.0"),
            true,
            &DeployDecision::UpToDate,
            target
        ));
    }
}
