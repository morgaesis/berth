use crate::config::{KubernetesPodRuntime, PodmanRuntime, Runtime};
use serde_json::Value;
use std::process::Command;

const DEFAULT_IDLE_SHUTDOWN_SECONDS: u64 = 3600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDiscovery {
    pub auto_enabled: bool,
    pub podman: ToolStatus,
    pub kubernetes: KubernetesStatus,
    pub default_runtime: Runtime,
    pub default_idle_shutdown_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolStatus {
    pub binary: String,
    pub available: bool,
    pub healthy: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KubernetesStatus {
    pub kubectl: ToolStatus,
    pub minikube: ToolStatus,
    pub namespace: String,
    pub runtime: Option<KubernetesPodRuntime>,
}

impl LocalDiscovery {
    pub fn discover() -> Self {
        if auto_discovery_disabled() {
            return Self::disabled();
        }

        let podman = discover_podman();
        let kubernetes = discover_kubernetes();
        let default_runtime = if podman.available && podman.healthy {
            Runtime::Podman(PodmanRuntime {
                ephemeral: true,
                ..PodmanRuntime::default()
            })
        } else {
            Runtime::Bare
        };

        Self {
            auto_enabled: true,
            podman,
            kubernetes,
            default_runtime,
            default_idle_shutdown_seconds: Some(DEFAULT_IDLE_SHUTDOWN_SECONDS),
        }
    }

    fn disabled() -> Self {
        Self {
            auto_enabled: false,
            podman: ToolStatus::disabled("podman"),
            kubernetes: KubernetesStatus {
                kubectl: ToolStatus::disabled("kubectl"),
                minikube: ToolStatus::disabled("minikube"),
                namespace: default_namespace(),
                runtime: None,
            },
            default_runtime: Runtime::Bare,
            default_idle_shutdown_seconds: None,
        }
    }
}

impl ToolStatus {
    fn disabled(binary: &str) -> Self {
        Self {
            binary: binary.to_string(),
            available: false,
            healthy: false,
            detail: "auto-discovery disabled".to_string(),
        }
    }
}

pub fn default_local_runtime() -> Runtime {
    LocalDiscovery::discover().default_runtime
}

pub fn default_idle_shutdown_seconds() -> Option<u64> {
    LocalDiscovery::discover().default_idle_shutdown_seconds
}

pub fn podman_userns_arg(binary: &str, configured: Option<&str>) -> Option<String> {
    if let Some(value) = configured {
        let trimmed = value.trim();
        return (!trimmed.is_empty()).then(|| format!("--userns={trimmed}"));
    }

    default_podman_userns(binary).map(|value| format!("--userns={value}"))
}

fn default_podman_userns(binary: &str) -> Option<&'static str> {
    if std::env::var_os("BERTH_FAKE_EXEC_LOG").is_some() {
        return Some("keep-id");
    }

    if podman_keep_id_works(binary) {
        Some("keep-id")
    } else {
        None
    }
}

fn auto_discovery_disabled() -> bool {
    std::env::var("BERTH_AUTO_DISCOVERY").is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
    })
}

fn discover_podman() -> ToolStatus {
    if which::which("podman").is_err() {
        return ToolStatus {
            binary: "podman".to_string(),
            available: false,
            healthy: false,
            detail: "not found on PATH".to_string(),
        };
    }

    match Command::new("podman")
        .args(["info", "--format", "{{.Host.Security.Rootless}}"])
        .output()
    {
        Ok(output) if output.status.success() => {
            let rootless = String::from_utf8_lossy(&output.stdout)
                .trim()
                .eq_ignore_ascii_case("true");
            ToolStatus {
                binary: "podman".to_string(),
                available: true,
                healthy: rootless,
                detail: if rootless {
                    "available, rootless".to_string()
                } else {
                    "available, not rootless".to_string()
                },
            }
        }
        Ok(output) => ToolStatus {
            binary: "podman".to_string(),
            available: true,
            healthy: false,
            detail: format!(
                "available, health probe failed with status {}",
                output.status
            ),
        },
        Err(error) => ToolStatus {
            binary: "podman".to_string(),
            available: true,
            healthy: false,
            detail: format!("available, health probe failed: {error}"),
        },
    }
}

fn podman_keep_id_works(binary: &str) -> bool {
    Command::new(binary)
        .args([
            "run",
            "--rm",
            "--userns=keep-id",
            "docker.io/library/alpine:latest",
            "true",
        ])
        .output()
        .is_ok_and(|output| output.status.success())
}

fn discover_kubernetes() -> KubernetesStatus {
    let kubectl = discover_simple_tool("kubectl");
    let minikube = discover_minikube();
    let namespace = default_namespace();
    let runtime = if kubectl.available && minikube.available && minikube.healthy {
        Some(KubernetesPodRuntime {
            namespace: Some(namespace.clone()),
            ephemeral: true,
            ..KubernetesPodRuntime::default()
        })
    } else {
        None
    };

    KubernetesStatus {
        kubectl,
        minikube,
        namespace,
        runtime,
    }
}

fn discover_simple_tool(binary: &str) -> ToolStatus {
    if which::which(binary).is_err() {
        return ToolStatus {
            binary: binary.to_string(),
            available: false,
            healthy: false,
            detail: "not found on PATH".to_string(),
        };
    }

    ToolStatus {
        binary: binary.to_string(),
        available: true,
        healthy: true,
        detail: "available".to_string(),
    }
}

fn discover_minikube() -> ToolStatus {
    if which::which("minikube").is_err() {
        return ToolStatus {
            binary: "minikube".to_string(),
            available: false,
            healthy: false,
            detail: "not found on PATH".to_string(),
        };
    }

    match Command::new("minikube")
        .args(["profile", "list", "-o", "json"])
        .output()
    {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let rootless_podman =
                minikube_rootless_podman(&stdout) || minikube_configured_for_rootless_podman();
            ToolStatus {
                binary: "minikube".to_string(),
                available: true,
                healthy: rootless_podman,
                detail: if rootless_podman {
                    "available, rootless Podman profile or config detected".to_string()
                } else {
                    "available, no rootless Podman profile or config detected".to_string()
                },
            }
        }
        Ok(output) => ToolStatus {
            binary: "minikube".to_string(),
            available: true,
            healthy: false,
            detail: format!(
                "available, profile probe failed with status {}",
                output.status
            ),
        },
        Err(error) => ToolStatus {
            binary: "minikube".to_string(),
            available: true,
            healthy: false,
            detail: format!("available, profile probe failed: {error}"),
        },
    }
}

fn minikube_rootless_podman(stdout: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(stdout) else {
        return stdout.to_ascii_lowercase().contains("podman")
            && stdout.to_ascii_lowercase().contains("rootless");
    };

    json_contains_rootless_podman(&value)
}

fn minikube_configured_for_rootless_podman() -> bool {
    minikube_config_value_is("driver", "podman") && minikube_config_value_is("rootless", "true")
}

fn minikube_config_value_is(property: &str, expected: &str) -> bool {
    let Ok(output) = Command::new("minikube")
        .args(["config", "get", property])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }

    String::from_utf8_lossy(&output.stdout)
        .trim()
        .eq_ignore_ascii_case(expected)
}

fn json_contains_rootless_podman(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            let driver_is_podman = map.iter().any(|(key, value)| {
                key.eq_ignore_ascii_case("driver") && json_has_text(value, "podman")
            });
            let rootless = map.iter().any(|(key, value)| {
                key.to_ascii_lowercase().contains("rootless") && matches!(value, Value::Bool(true))
            });

            (driver_is_podman && rootless) || map.values().any(json_contains_rootless_podman)
        }
        Value::Array(values) => values.iter().any(json_contains_rootless_podman),
        _ => false,
    }
}

fn json_has_text(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(text) => text.eq_ignore_ascii_case(needle),
        _ => false,
    }
}

fn default_namespace() -> String {
    "berth".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minikube_rootless_podman_profiles() {
        let json = r#"{
          "valid": [
            {
              "Name": "minikube",
              "Config": {
                "Driver": "podman",
                "Rootless": true
              }
            }
          ]
        }"#;

        assert!(minikube_rootless_podman(json));
    }

    #[test]
    fn rejects_minikube_non_rootless_profiles() {
        let json = r#"{
          "valid": [
            {
              "Name": "minikube",
              "Config": {
                "Driver": "podman",
                "Rootless": false
              }
            }
          ]
        }"#;

        assert!(!minikube_rootless_podman(json));
    }
}
