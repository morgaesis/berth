use anyhow::Result;
use std::env;
use std::fs;
use std::io::Write;

const HOSTS_MARKER_START: &str = "# === BERTH START ===";
const HOSTS_MARKER_END: &str = "# === BERTH END ===";

fn skip_hosts() -> bool {
    env::var("BERTH_SKIP_HOSTS").is_ok()
}

fn hosts_path() -> String {
    env::var("BERTH_HOSTS_PATH").unwrap_or_else(|_| "/etc/hosts".to_string())
}

pub fn add_entry(name: &str) -> Result<()> {
    if skip_hosts() {
        return Ok(());
    }

    let hosts_path = hosts_path();
    let content = fs::read_to_string(&hosts_path)?;

    if content.contains(&format!(" {}.berth", name)) {
        return Ok(());
    }

    let entries: Vec<String> = if content.contains(HOSTS_MARKER_START) {
        content.lines().map(|l| l.to_string()).collect()
    } else {
        let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
        lines.push(String::new());
        lines.push(HOSTS_MARKER_START.to_string());
        lines.push(HOSTS_MARKER_END.to_string());
        lines
    };

    let mut new_entries = entries.clone();
    let insert_pos = new_entries
        .iter()
        .position(|l| l == HOSTS_MARKER_START)
        .map(|p| p + 1)
        .unwrap_or(new_entries.len() - 1);

    let entry = format!("127.0.0.1 {}.berth", name);
    if !new_entries.contains(&entry) {
        new_entries.insert(insert_pos, entry);
    }

    let temp_path = "/tmp/berth_hosts_tmp";
    let mut file = fs::File::create(temp_path)?;
    for line in &new_entries {
        writeln!(file, "{}", line)?;
    }

    let status = std::process::Command::new("sudo")
        .args(["cp", temp_path, &hosts_path])
        .status()?;

    if !status.success() {
        anyhow::bail!("Failed to update hosts file");
    }

    Ok(())
}

pub fn remove_entry(name: &str) -> Result<()> {
    if skip_hosts() {
        return Ok(());
    }

    let hosts_path = hosts_path();
    let content = fs::read_to_string(&hosts_path)?;

    let entries: Vec<String> = content
        .lines()
        .filter(|l| !l.contains(&format!("{}.berth", name)))
        .map(|l| l.to_string())
        .collect();

    let temp_path = "/tmp/berth_hosts_tmp";
    let mut file = fs::File::create(temp_path)?;
    for line in &entries {
        writeln!(file, "{}", line)?;
    }

    let status = std::process::Command::new("sudo")
        .args(["cp", temp_path, &hosts_path])
        .status()?;

    if !status.success() {
        anyhow::bail!("Failed to update hosts file");
    }

    Ok(())
}

pub fn clean() -> Result<()> {
    if skip_hosts() {
        return Ok(());
    }

    let hosts_path = hosts_path();
    let content = fs::read_to_string(&hosts_path)?;

    let mut in_berth_section = false;
    let entries: Vec<String> = content
        .lines()
        .filter(|l| {
            if *l == HOSTS_MARKER_START {
                in_berth_section = true;
                true
            } else if *l == HOSTS_MARKER_END {
                in_berth_section = false;
                true
            } else {
                !in_berth_section
            }
        })
        .map(|l| l.to_string())
        .collect();

    let temp_path = "/tmp/berth_hosts_tmp";
    let mut file = fs::File::create(temp_path)?;
    for line in &entries {
        writeln!(file, "{}", line)?;
    }

    let status = std::process::Command::new("sudo")
        .args(["cp", temp_path, &hosts_path])
        .status()?;

    if !status.success() {
        anyhow::bail!("Failed to update hosts file");
    }

    Ok(())
}

pub fn check_dnsmasq_available() -> bool {
    which::which("dnsmasq").is_ok()
}

pub fn print_dnsmasq_setup() {
    println!("# To enable *.berth DNS resolution without sudo, add to /etc/dnsmasq.d/berth:");
    println!("address=/berth/127.0.0.1");
    println!("");
    println!("# Then restart dnsmasq:");
    println!("sudo systemctl restart dnsmasq");
    println!("");
    println!("# Or if using systemd-resolved, add to /etc/systemd/resolved.conf:");
    println!("[Resolve]");
    println!("DNS=127.0.0.1");
    println!("Domains=~berth");
}

pub fn install() -> Result<()> {
    println!("=== Berth DNS Setup ===");
    println!("");
    println!("This will add a wildcard entry to /etc/hosts for *.berth domains.");
    println!("This requires sudo access and will prompt for your password.");
    println!("");
    println!("Alternative: Use dnsmasq or systemd-resolved for DNS-based resolution.");
    println!("Run 'berth hosts setup-dns' for instructions.");
    println!("");

    let hosts_path = hosts_path();
    let content = fs::read_to_string(&hosts_path)?;

    if content.contains("*.berth") || content.contains(".berth") && content.contains("127.0.0.1") {
        println!("Wildcard entry already exists in {}.", hosts_path);
        return Ok(());
    }

    let entries: Vec<String> = if content.contains(HOSTS_MARKER_START) {
        content.lines().map(|l| l.to_string()).collect()
    } else {
        let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
        lines.push(String::new());
        lines.push(HOSTS_MARKER_START.to_string());
        lines.push(HOSTS_MARKER_END.to_string());
        lines
    };

    let mut new_entries = entries.clone();
    let insert_pos = new_entries
        .iter()
        .position(|l| l == HOSTS_MARKER_START)
        .map(|p| p + 1)
        .unwrap_or(new_entries.len() - 1);

    let entry = "127.0.0.1 *.berth".to_string();
    if !new_entries.contains(&entry) {
        new_entries.insert(insert_pos, entry);
    }

    let temp_path = "/tmp/berth_hosts_tmp";
    let mut file = fs::File::create(temp_path)?;
    for line in &new_entries {
        writeln!(file, "{}", line)?;
    }

    println!("Adding wildcard entry for *.berth to {}...", hosts_path);
    let status = std::process::Command::new("sudo")
        .args(["cp", temp_path, &hosts_path])
        .status()?;

    if !status.success() {
        anyhow::bail!("Failed to update hosts file. Please run with sudo or set up dnsmasq.");
    }

    println!("Done! *.berth now resolves to 127.0.0.1");
    println!("You can now access workspaces at http://<name>.berth:<port>");

    Ok(())
}
