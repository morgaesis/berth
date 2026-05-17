use anyhow::{bail, Result};
use berth::config::Config;
use berth::deploy::{self, DeployDecision};
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

pub async fn run(host: String, tag: Option<String>, force: bool) -> Result<()> {
    deploy::freshness::warn_if_stale().await;

    let mut config = Config::load()?;
    let local_version = env!("CARGO_PKG_VERSION").to_string();
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
    eprintln!("  local:  {local_os} / {local_arch}  ({})", local_version);
    eprintln!("  remote: {} / {}  ({})", env.os, env.arch, remote_state);

    let decision = deploy::decide(&env, &local_version);
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
        DeployDecision::Deploy { target, .. } => target,
    };
    eprintln!("  target: {target}");

    let tag = tag.unwrap_or_else(|| format!("v{local_version}"));
    let info = deploy::ensure_deployed(&host, &tag, target).await?;
    deploy::record_trust(&mut config, &host, &info)?;

    eprintln!(
        "berth: deployed v{} to {}:{}  (host added to trusted_hosts)",
        info.version,
        host,
        info.remote_path.display()
    );
    Ok(())
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
