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

/// Human-readable local host architecture for deploy-plan output. Unlike
/// `std::env::consts::ARCH` (fixed at compile time), this reports the
/// *machine's* architecture even when the binary runs under emulation —
/// an x86_64 build on Windows-on-ARM would otherwise claim x86_64.
pub fn local_arch_description() -> String {
    let compiled = std::env::consts::ARCH;
    match native_machine_arch() {
        Some(native) if native != compiled => {
            format!("{native} ({compiled} binary under emulation)")
        }
        _ => compiled.to_string(),
    }
}

#[cfg(windows)]
fn native_machine_arch() -> Option<&'static str> {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn IsWow64Process2(
            process: *mut c_void,
            process_machine: *mut u16,
            native_machine: *mut u16,
        ) -> i32;
    }
    let mut process_machine = 0u16;
    let mut native_machine = 0u16;
    let ok = unsafe {
        IsWow64Process2(
            GetCurrentProcess(),
            &mut process_machine,
            &mut native_machine,
        )
    };
    if ok == 0 {
        return None;
    }
    arch_from_image_file_machine(native_machine)
}

/// Map an IMAGE_FILE_MACHINE_* constant to rustc arch naming.
#[cfg(windows)]
fn arch_from_image_file_machine(machine: u16) -> Option<&'static str> {
    match machine {
        0x8664 => Some("x86_64"),  // IMAGE_FILE_MACHINE_AMD64
        0xAA64 => Some("aarch64"), // IMAGE_FILE_MACHINE_ARM64
        0x014C => Some("x86"),     // IMAGE_FILE_MACHINE_I386
        _ => None,
    }
}

#[cfg(not(windows))]
fn native_machine_arch() -> Option<&'static str> {
    None
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

    #[cfg(windows)]
    #[test]
    fn image_file_machine_maps_to_rustc_arch_names() {
        assert_eq!(arch_from_image_file_machine(0x8664), Some("x86_64"));
        assert_eq!(arch_from_image_file_machine(0xAA64), Some("aarch64"));
        assert_eq!(arch_from_image_file_machine(0x014C), Some("x86"));
        assert_eq!(arch_from_image_file_machine(0xFFFF), None);
    }
}
