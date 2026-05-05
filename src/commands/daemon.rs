use crate::commands::reap;
use anyhow::{bail, Result};
use std::time::Duration;

const DEFAULT_INTERVAL_SECONDS: u64 = 300;

pub async fn run(interval_seconds: Option<u64>, once: bool) -> Result<()> {
    let interval_seconds = interval_seconds.unwrap_or(DEFAULT_INTERVAL_SECONDS);
    if interval_seconds == 0 {
        bail!("daemon interval must be greater than zero seconds");
    }

    println!(
        "Berth daemon running in foreground; idle reaper interval is {} second(s).",
        interval_seconds
    );

    run_iteration().await?;

    if once {
        println!("Berth daemon one-shot run complete.");
        return Ok(());
    }

    let interval = Duration::from_secs(interval_seconds);
    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => run_iteration().await?,
            signal = tokio::signal::ctrl_c() => {
                signal?;
                println!("Berth daemon shutting down.");
                return Ok(());
            }
        }
    }
}

async fn run_iteration() -> Result<()> {
    let summary = reap::run_once().await?;
    reap::print_summary(summary);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_zero_interval() {
        let error = run(Some(0), true).await.unwrap_err();
        assert!(error.to_string().contains("greater than zero"));
    }
}
