mod cli;
mod commands;

use clap::Parser;
use cli::Cli;

/// File-backed log location. Shared by client (this binary) and
/// supervisor — operators looking at a hang can `cat` this and see
/// what was happening on both sides.
fn log_file_path() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("BERTH_LOG_FILE") {
        return Some(std::path::PathBuf::from(p));
    }
    let base = std::env::var("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .ok()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("state")))?;
    Some(base.join("berth").join("log").join("berth.log"))
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let filter = match cli.log_filter() {
        Some(directive) => tracing_subscriber::EnvFilter::new(directive),
        None => tracing_subscriber::EnvFilter::from_default_env(),
    };
    // Two layers, two filters: stderr follows the user's -v setting,
    // file layer ALWAYS captures info+ so `berth logs` shows useful
    // detail even after a default-verbosity run. Without this, sharing
    // logs back to a debugging session would require running everything
    // under -vv preemptively.
    use tracing_subscriber::{
        fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer,
    };
    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(filter)
        .boxed();
    let file_layer: Option<Box<dyn Layer<_> + Send + Sync>> = log_file_path().and_then(|path| {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()?;
        Some(
            fmt::layer()
                .with_ansi(false)
                .with_writer(std::sync::Arc::new(f))
                .with_filter(EnvFilter::new("berth=info"))
                .boxed(),
        )
    });
    let registry = tracing_subscriber::registry().with(stderr_layer);
    match file_layer {
        Some(file_layer) => registry.with(file_layer).init(),
        None => registry.init(),
    }
    tracing::info!(
        argv = ?std::env::args().collect::<Vec<_>>(),
        version = env!("CARGO_PKG_VERSION"),
        pid = std::process::id(),
        "berth invocation"
    );

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
