use anyhow::Result;
use tokio::signal;

pub async fn run(ports: Vec<u16>) -> Result<()> {
    if ports.is_empty() {
        println!("Agent started. Waiting for commands...");
    } else {
        println!("Agent forwarding ports: {:?}", ports);
    }

    let ctrl_c = signal::ctrl_c();
    tokio::select! {
        _ = ctrl_c => {
            println!("\nAgent stopped.");
        }
    }

    Ok(())
}
