// Terminal control signals for new-tab auto-entry.
//
// Cascade of signals (best to worst), all emitted at enter time:
//   1. OSC 1337 SetUserVar=BERTH_PROJECT=<b64> — semantic, pane-local in
//      WezTerm/iTerm2. Doesn't propagate to new tabs in either emulator,
//      but cheap to emit and future-proof.
//   2. OSC 7 cwd report pointing at a per-workspace marker directory under
//      $XDG_STATE_HOME/berth/active/. This is the only mechanism that
//      reliably propagates to new tabs across common emulators (WezTerm,
//      iTerm2, GNOME Terminal, Konsole, kitty, Alacritty), because they
//      inherit OSC-7-reported cwd for newly spawned tabs.
//   3. OSC 2 / OSC 1 title — visible breadcrumb.
//
// The marker directory is created but never deleted by berth: shells that
// inherited it as cwd would otherwise hit a `getwd()` failure on every
// prompt. The shell-init detector reads the path, extracts the workspace
// name, and `cd`s away to $HOME before invoking `berth enter`, so the
// marker cwd is transient (≪1ms) from the user's perspective.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

/// Per-workspace marker directory used as the OSC 7 cwd target.
/// New tabs inherit this path as their starting cwd; the shell hook
/// decodes the trailing component back into a workspace name.
pub fn marker_dir(workspace: &str) -> PathBuf {
    let base = if let Ok(dir) = std::env::var("XDG_STATE_HOME") {
        PathBuf::from(dir)
    } else {
        dirs::state_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("state")))
            .unwrap_or_else(|| PathBuf::from("/tmp"))
    };
    base.join("berth")
        .join("active")
        .join(workspace.replace('/', "_"))
}

fn percent_encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn b64(input: &str) -> String {
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHA[((n >> 18) & 0x3F) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHA[((n >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHA[(n & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Title strings are short, single-line workspace names. Reject control
/// characters defensively to keep the OSC 2 / OSC 1 payload well-formed —
/// `validate_workspace_name` already enforces this at intake, but treating
/// the boundary defensively is cheap.
fn title_safe(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() && *c != '\x1b')
        .collect()
}

/// Emit signals announcing a workspace entry.
pub fn emit_enter_signals(workspace: &str) {
    let dir = marker_dir(workspace);
    let _ = fs::create_dir_all(&dir);

    let mut out = io::stdout().lock();
    let safe = title_safe(workspace);

    // 1. Pane-scoped user variable (WezTerm/iTerm2). Cheap; harmless on
    //    emulators that ignore it.
    let _ = write!(out, "\x1b]1337;SetUserVar=BERTH_PROJECT={}\x07", b64(&safe));

    // 2. OSC 7 cwd report → new tabs inherit this path. The shell-init
    //    detector reads it and `cd`s away to $HOME before doing anything
    //    user-visible, so the cwd pollution is transient.
    let path = dir.to_string_lossy();
    let _ = write!(
        out,
        "\x1b]7;file://localhost{}\x1b\\",
        percent_encode_path(&path)
    );

    // 3. Title — human breadcrumb.
    let _ = write!(out, "\x1b]2;berth: {safe}\x07");
    let _ = write!(out, "\x1b]1;berth: {safe}\x07");
    let _ = out.flush();
}

/// Emit signals that clear the workspace breadcrumb so new tabs spawned
/// after a clean exit do not auto-enter. The marker directory is left in
/// place: any shell still cwd'd inside it would otherwise lose its $PWD.
pub fn emit_exit_signals(_workspace: &str) {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    let mut out = io::stdout().lock();

    let _ = write!(out, "\x1b]1337;SetUserVar=BERTH_PROJECT=\x07");
    let _ = write!(
        out,
        "\x1b]7;file://localhost{}\x1b\\",
        percent_encode_path(&home)
    );
    let _ = write!(out, "\x1b]2;\x07");
    let _ = write!(out, "\x1b]1;\x07");
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64_basic() {
        assert_eq!(b64(""), "");
        assert_eq!(b64("f"), "Zg==");
        assert_eq!(b64("fo"), "Zm8=");
        assert_eq!(b64("foo"), "Zm9v");
        assert_eq!(b64("foob"), "Zm9vYg==");
    }

    #[test]
    fn title_safe_strips_controls_and_escape() {
        assert_eq!(title_safe("foo\x1bbar"), "foobar");
        assert_eq!(title_safe("hi\nworld"), "hiworld");
        assert_eq!(title_safe("org/project"), "org/project");
    }
}
