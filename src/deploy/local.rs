//! Map the *local* OS/arch to a release-target triple. Used by the
//! freshness checker so it can suggest the artifact a user would download
//! to update their own machine.

/// Return the release target triple matching the running binary's host,
/// or `None` if we don't ship a build for it. Mirrors the host probe's
/// `target_triple` but reads from `std::env::consts`.
pub fn local_target_triple() -> Option<&'static str> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    match (os, arch) {
        // Pick musl for Linux: a musl-static binary runs on glibc systems
        // too, so it's the safest single recommendation regardless of the
        // user's libc.
        ("linux", "x86_64") => Some("x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-musl"),
        ("linux", "arm") => Some("armv7-unknown-linux-musleabihf"),
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_target_triple_returns_something_or_explicitly_none() {
        // We just verify the call works without panicking on this host;
        // we can't assert a specific value because tests run on any host.
        let _ = local_target_triple();
    }
}
