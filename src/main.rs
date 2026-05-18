mod cli;
mod commands;

use clap::Parser;
use cli::Cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let filter = match cli.log_filter() {
        Some(directive) => tracing_subscriber::EnvFilter::new(directive),
        None => tracing_subscriber::EnvFilter::from_default_env(),
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();
    cli.run().await
}
