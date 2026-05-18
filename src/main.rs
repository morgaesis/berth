mod cli;
mod commands;

use clap::Parser;
use cli::Cli;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let filter = match cli.log_filter() {
        Some(directive) => tracing_subscriber::EnvFilter::new(directive),
        None => tracing_subscriber::EnvFilter::from_default_env(),
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // Force-exit instead of returning so the tokio runtime doesn't block
    // waiting on blocking workers (reqwest connection pools, inherited
    // stdio from spawned ssh, etc.) that have outlived their useful work.
    // We've already finished cli.run(); user-visible side effects are
    // done. The kernel handles fd cleanup.
    let code = match cli.run().await {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("Error: {err:#}");
            1
        }
    };
    // Bypass `std::process::exit` -> libc `exit()` -> atexit handlers /
    // stdio flush. We have observed exit() hanging on systems where one
    // of our spawned children (ssh in particular) left stderr in a state
    // that blocks libc's final fflush. `_exit` is a direct kernel call —
    // no atexit, no flush, no thread-join. We've already user-visibly
    // finished by this point (the trace lines above made it out before
    // we got here), so dropping pending stdio is safe.
    unsafe { libc::_exit(code) }
}
