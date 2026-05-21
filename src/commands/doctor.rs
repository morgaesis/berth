use anyhow::Result;
use berth::config::Runtime;
use berth::discovery::LocalDiscovery;
use colored::Colorize;
use std::path::PathBuf;

pub async fn run() -> Result<()> {
    let discovery = LocalDiscovery::discover();

    println!("{}", "Berth shell integration".bold());
    let hook = check_shell_hook();
    println!("  Detected shell: {}", hook.shell_label.cyan());
    let hook_status = if hook.installed {
        "yes".green().to_string()
    } else {
        format!(
            "{} — add `eval \"$(berth shell init)\"` to your rc file",
            "no".yellow()
        )
    };
    println!("  Hook installed: {}", hook_status);
    println!("  berth on PATH: {}", hook.path_label.dimmed());
    println!();

    println!("{}", "Berth discovery".bold());
    println!("Auto-discovery: {}", enabled_label(discovery.auto_enabled));
    println!(
        "Default local runtime: {}",
        runtime_label(&discovery.default_runtime).cyan()
    );
    let idle_label = discovery
        .default_idle_shutdown_seconds
        .map(|seconds| format!("{seconds}s").cyan().to_string())
        .unwrap_or_else(|| "disabled".dimmed().to_string());
    println!("Default idle shutdown: {idle_label}");
    println!(
        "Podman: {} ({})",
        status_label(discovery.podman.available, discovery.podman.healthy),
        discovery.podman.detail.dimmed()
    );
    println!(
        "kubectl: {} ({})",
        status_label(
            discovery.kubernetes.kubectl.available,
            discovery.kubernetes.kubectl.healthy
        ),
        discovery.kubernetes.kubectl.detail.dimmed()
    );
    println!(
        "minikube: {} ({})",
        status_label(
            discovery.kubernetes.minikube.available,
            discovery.kubernetes.minikube.healthy
        ),
        discovery.kubernetes.minikube.detail.dimmed()
    );

    if let Some(runtime) = discovery.kubernetes.runtime {
        println!(
            "Kubernetes pod defaults: {} namespace={} image={}",
            "available".green(),
            runtime
                .namespace
                .unwrap_or_else(|| "default".to_string())
                .cyan(),
            runtime.image.cyan()
        );
    } else {
        println!(
            "Kubernetes pod defaults: {}",
            "unavailable".dimmed()
        );
    }

    Ok(())
}

fn enabled_label(value: bool) -> colored::ColoredString {
    if value {
        "enabled".green()
    } else {
        "disabled".dimmed()
    }
}

fn status_label(available: bool, healthy: bool) -> colored::ColoredString {
    match (available, healthy) {
        (true, true) => "ready".green(),
        (true, false) => "available".yellow(),
        (false, _) => "missing".dimmed(),
    }
}

fn runtime_label(runtime: &Runtime) -> &'static str {
    match runtime {
        Runtime::Auto => "auto",
        Runtime::Bare => "bare",
        Runtime::Podman(_) => "podman",
        Runtime::KubernetesPod(_) => "kubernetes-pod",
    }
}

struct ShellHookCheck {
    shell_label: String,
    installed: bool,
    path_label: String,
}

/// Look for the berth shell-hook in the user's rc file. We grep for the
/// generated-marker comment rather than the eval invocation itself, so
/// equivalent forms (sourcing a file that sources berth, etc.) are all
/// detected.
fn check_shell_hook() -> ShellHookCheck {
    let shell = std::env::var("SHELL").ok();
    let (shell_label, rc_candidates) = match shell
        .as_deref()
        .and_then(|s| std::path::Path::new(s).file_name().and_then(|n| n.to_str()))
    {
        Some("zsh") => (
            "zsh".to_string(),
            vec![dot_home(".zshrc"), dot_home(".zprofile")],
        ),
        Some("bash") => (
            "bash".to_string(),
            vec![
                dot_home(".bashrc"),
                dot_home(".bash_profile"),
                dot_home(".profile"),
            ],
        ),
        Some(other) => (other.to_string(), vec![]),
        None => ("unknown".to_string(), vec![]),
    };

    let installed = rc_candidates.iter().any(rc_file_has_hook);

    let path_label = match which("berth") {
        Some(p) => p.display().to_string(),
        None => "not found on PATH".to_string(),
    };

    ShellHookCheck {
        shell_label,
        installed,
        path_label,
    }
}

fn dot_home(name: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(name)
}

fn rc_file_has_hook(path: &PathBuf) -> bool {
    let Ok(s) = std::fs::read_to_string(path) else {
        return false;
    };
    s.contains("berth shell init") || s.contains("berth shell-init")
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if std::fs::metadata(&cand)
            .map(|m| m.is_file())
            .unwrap_or(false)
        {
            return Some(cand);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn rc_file_detects_either_init_form() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("rc");
        let mut f = std::fs::File::create(&p).unwrap();
        writeln!(f, "# unrelated").unwrap();
        writeln!(f, "eval \"$(berth shell init)\"").unwrap();
        assert!(rc_file_has_hook(&p));

        let p2 = dir.path().join("rc2");
        let mut f2 = std::fs::File::create(&p2).unwrap();
        writeln!(f2, "eval \"$(berth shell-init)\"").unwrap();
        assert!(rc_file_has_hook(&p2));

        let p3 = dir.path().join("rc3");
        std::fs::write(&p3, b"# no hook\n").unwrap();
        assert!(!rc_file_has_hook(&p3));
    }

    #[test]
    fn rc_file_handles_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!rc_file_has_hook(&dir.path().join("does-not-exist")));
    }
}
