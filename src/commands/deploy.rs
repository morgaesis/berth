use anyhow::{bail, Result};
use berth::config::Config;
use berth::deploy::{self, DeployDecision};

pub async fn run(host: String, tag: Option<String>, force: bool) -> Result<()> {
    let mut config = Config::load()?;
    let local_version = env!("CARGO_PKG_VERSION").to_string();

    eprintln!("berth: probing {host}…");
    let env = deploy::probe(&host).await?;
    eprintln!(
        "berth: remote {host} → {} / {} ({})",
        env.os,
        env.arch,
        env.berth_version.as_deref().unwrap_or("no existing berth")
    );

    let decision = deploy::decide(&env, &local_version);
    let target = match &decision {
        DeployDecision::UnsupportedArch { os, arch } => {
            bail!(
                "no pre-built berth for {os}/{arch}; install tmux or screen on {host} \
                 and use `berth enter --plain --remote {host} <ws>` for non-resumable shells"
            );
        }
        DeployDecision::UpToDate => {
            // target_triple() must succeed here because UpToDate already
            // implies the arch is in the build matrix. Surface a clear
            // internal-bug message instead of an `unwrap()` panic if a
            // future refactor breaks that invariant.
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
                    "berth: {host} already has berth {local_version}; nothing to do \
                     (use --force to redeploy)"
                );
                return Ok(());
            }
            t
        }
        DeployDecision::Deploy { target, .. } => target,
    };

    let tag = tag.unwrap_or_else(|| format!("v{local_version}"));
    eprintln!("berth: fetching berth-{target} from release {tag} and pushing to {host}…");
    let info = deploy::ensure_deployed(&host, &tag, target).await?;
    deploy::record_trust(&mut config, &host, &info)?;

    println!(
        "berth: deployed berth-{} to {}:{} (trusted_hosts updated)",
        info.version,
        host,
        info.remote_path.display()
    );
    Ok(())
}
