use anyhow::Result;
use berth::config::Runtime;
use berth::discovery::LocalDiscovery;

pub async fn run() -> Result<()> {
    let discovery = LocalDiscovery::discover();

    println!("Berth discovery");
    println!("Auto-discovery: {}", enabled(discovery.auto_enabled));
    println!(
        "Default local runtime: {}",
        runtime_label(&discovery.default_runtime)
    );
    println!(
        "Default idle shutdown: {}",
        discovery
            .default_idle_shutdown_seconds
            .map(|seconds| format!("{seconds}s"))
            .unwrap_or_else(|| "disabled".to_string())
    );
    println!(
        "Podman: {} ({})",
        status_label(discovery.podman.available, discovery.podman.healthy),
        discovery.podman.detail
    );
    println!(
        "kubectl: {} ({})",
        status_label(
            discovery.kubernetes.kubectl.available,
            discovery.kubernetes.kubectl.healthy
        ),
        discovery.kubernetes.kubectl.detail
    );
    println!(
        "minikube: {} ({})",
        status_label(
            discovery.kubernetes.minikube.available,
            discovery.kubernetes.minikube.healthy
        ),
        discovery.kubernetes.minikube.detail
    );

    if let Some(runtime) = discovery.kubernetes.runtime {
        println!(
            "Kubernetes pod defaults: available namespace={} image={}",
            runtime.namespace.unwrap_or_else(|| "default".to_string()),
            runtime.image
        );
    } else {
        println!("Kubernetes pod defaults: unavailable");
    }

    Ok(())
}

fn enabled(value: bool) -> &'static str {
    if value {
        "enabled"
    } else {
        "disabled"
    }
}

fn status_label(available: bool, healthy: bool) -> &'static str {
    match (available, healthy) {
        (true, true) => "ready",
        (true, false) => "available",
        (false, _) => "missing",
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
