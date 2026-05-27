use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=Cargo.lock");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-changed=.git/info/exclude");
    println!("cargo:rerun-if-env-changed=BERTH_BUILD_INPUT_REFRESH");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=tests");
    for path in tracked_inputs() {
        println!("cargo:rerun-if-changed={path}");
    }

    let sha = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .and_then(|out| out.status.success().then_some(out.stdout))
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    let dirty = Command::new("git")
        .args([
            "status",
            "--porcelain",
            "--untracked-files=normal",
            "--",
            "Cargo.toml",
            "Cargo.lock",
            "build.rs",
            "src",
            "tests",
        ])
        .output()
        .map(|out| !out.stdout.is_empty())
        .unwrap_or(false);

    let build_id = if dirty { format!("{sha}-dirty") } else { sha };
    if let Ok(target) = std::env::var("TARGET") {
        println!("cargo:rustc-env=BERTH_BUILD_TARGET={target}");
    }
    println!("cargo:rustc-env=BERTH_BUILD_ID={build_id}");
}

fn tracked_inputs() -> Vec<String> {
    let mut inputs = Vec::new();
    for args in [
        &[
            "ls-files",
            "Cargo.toml",
            "Cargo.lock",
            "build.rs",
            "src",
            "tests",
        ][..],
        &[
            "ls-files",
            "--others",
            "--exclude-standard",
            "Cargo.toml",
            "Cargo.lock",
            "build.rs",
            "src",
            "tests",
        ][..],
    ] {
        if let Some(out) = Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| String::from_utf8(out.stdout).ok())
        {
            inputs.extend(
                out.lines()
                    .filter(|line| !line.trim().is_empty())
                    .map(|line| line.to_string()),
            );
        }
    }
    inputs.sort();
    inputs.dedup();
    inputs
}
