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
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

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
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char)
            }
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

/// Inputs the shell hook needs to exactly replicate this `berth enter`
/// in a new tab: which workspace, which remote dir override (if any),
/// and which command override (if any). The hook serializes these into
/// a single `BERTH_SKIP_AUTO=1 command berth enter --new …` line that
/// it `eval`s on new-tab startup.
pub struct EnterSignal<'a> {
    pub workspace: &'a str,
    pub dir: Option<&'a str>,
    pub command: Option<&'a [String]>,
}

/// POSIX-safe single-quote escape. Each embedded `'` is broken out as
/// `'"'"'` so the result is always one shell word.
///
/// NUL and other ASCII control bytes are dropped — they can't appear in
/// a shell line (NUL truncates `eval` input on most shells; other
/// controls would corrupt the invoke file). User input that contains
/// them was almost certainly a mistake; silently filtering is safer
/// than letting them through.
fn shell_quote(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .filter(|c| !c.is_control() || *c == '\t')
        .collect();
    format!("'{}'", cleaned.replace('\'', "'\"'\"'"))
}

/// Write `contents` to `path` with mode 0600 and `O_NOFOLLOW` so that a
/// same-uid attacker can't pre-create the path as a symlink to a
/// sensitive file and have us clobber it. Best-effort: returns the
/// underlying io::Result; callers ignore failure (state-dir writes are
/// non-critical signals).
fn write_secure(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::fs::OpenOptions;
    // Pre-emptively remove any existing entry (including a symlink) so
    // O_NOFOLLOW + O_CREAT succeeds on the next open. `remove_file`
    // does not follow symlinks, so this only unlinks the link itself.
    let _ = fs::remove_file(path);
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    f.write_all(contents)
}

/// Build the exact line the new-tab hook should evaluate.
fn build_invoke_line(signal: &EnterSignal<'_>) -> String {
    let mut out = String::from("BERTH_SKIP_AUTO=1 command berth enter --new ");
    out.push_str(&shell_quote(signal.workspace));
    if let Some(d) = signal.dir {
        out.push_str(" --dir ");
        out.push_str(&shell_quote(d));
    }
    if let Some(argv) = signal.command {
        if !argv.is_empty() {
            out.push_str(" --");
            for arg in argv {
                out.push(' ');
                out.push_str(&shell_quote(arg));
            }
        }
    }
    out
}

/// Location of the global "last-active workspace" pointer. Single file,
/// last writer wins. Used by the shell hook as a fallback signal in
/// environments where OSC 7 cwd inheritance doesn't reach new tabs —
/// notably Windows Terminal + WSL, where the remote shell's chpwd
/// emits OSC 7 with remote paths that fail to chdir locally.
pub fn last_active_path() -> PathBuf {
    let base = if let Ok(dir) = std::env::var("XDG_STATE_HOME") {
        PathBuf::from(dir)
    } else {
        dirs::state_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("state")))
            .unwrap_or_else(|| PathBuf::from("/tmp"))
    };
    base.join("berth").join("last-active")
}

/// Emit signals announcing a workspace entry, plus persist the exact
/// re-invocation command so a new tab spawned from this one runs the
/// same workspace + dir + command instead of falling back to the
/// workspace's stored defaults.
pub fn emit_enter_signals(signal: &EnterSignal<'_>) {
    let dir = marker_dir(signal.workspace);
    let _ = fs::create_dir_all(&dir);
    // Canonical workspace name (slash-preserving) for the basename →
    // name fallback path.
    let _ = write_secure(&dir.join(".workspace"), signal.workspace.as_bytes());
    // Exact re-invocation line. Hook reads + evals this — write with
    // O_NOFOLLOW + mode 0600 so a same-uid process can't redirect the
    // write via a planted symlink or read what we wrote.
    let invoke = build_invoke_line(signal);
    let _ = write_secure(&dir.join(".invoke"), invoke.as_bytes());
    // Mirror to the global last-active pointer so new tabs that lose
    // cwd inheritance (WSL+WinTerm) can still find a usable signal.
    let last = last_active_path();
    if let Some(parent) = last.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = write_secure(&last, invoke.as_bytes());

    let mut out = io::stdout().lock();
    let safe = title_safe(signal.workspace);

    // 1. Pane-scoped user variable (WezTerm/iTerm2). Cheap; harmless on
    //    emulators that ignore it.
    let _ = write!(out, "\x1b]1337;SetUserVar=BERTH_PROJECT={}\x07", b64(&safe));

    // 2. OSC 7 cwd report → new tabs inherit this path on emulators
    //    that honor it. The shell-init detector reads it and `cd`s
    //    away to $HOME before doing anything user-visible.
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
/// The last-active pointer is removed so the fallback signal disappears.
pub fn emit_exit_signals(workspace: &str) {
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

    // Best-effort: remove the global last-active pointer ONLY if it
    // still refers to this workspace. Another sibling tab may have
    // entered a different workspace after us; unconditionally
    // deleting would silently break its new-tab fallback signal.
    let last = last_active_path();
    if let Ok(contents) = fs::read_to_string(&last) {
        let token = format!(" enter --new {}", shell_quote(workspace));
        if contents.contains(&token) {
            let _ = fs::remove_file(&last);
        }
    }
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

    #[test]
    fn shell_quote_handles_metacharacters_and_quotes() {
        assert_eq!(shell_quote("hello"), "'hello'");
        assert_eq!(shell_quote("a b c"), "'a b c'");
        assert_eq!(shell_quote("it's"), "'it'\"'\"'s'");
        assert_eq!(shell_quote("$(rm -rf /)"), "'$(rm -rf /)'");
        assert_eq!(shell_quote("`whoami`"), "'`whoami`'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn build_invoke_workspace_only() {
        let s = EnterSignal {
            workspace: "morgaesis/postil",
            dir: None,
            command: None,
        };
        assert_eq!(
            build_invoke_line(&s),
            "BERTH_SKIP_AUTO=1 command berth enter --new 'morgaesis/postil'"
        );
    }

    #[test]
    fn build_invoke_with_dir_and_command() {
        let cmd = vec!["zsh".to_string()];
        let s = EnterSignal {
            workspace: "morgaesis/postil",
            dir: Some("~/Projects/morgaesis/postil.dev"),
            command: Some(&cmd),
        };
        assert_eq!(
            build_invoke_line(&s),
            "BERTH_SKIP_AUTO=1 command berth enter --new 'morgaesis/postil' \
             --dir '~/Projects/morgaesis/postil.dev' -- 'zsh'"
        );
    }

    #[test]
    fn build_invoke_quotes_command_with_metacharacters() {
        let cmd = vec![
            "bash".to_string(),
            "-ic".to_string(),
            "sudo -u dev bash -ic 'cd app && assist'".to_string(),
        ];
        let s = EnterSignal {
            workspace: "acme/app",
            dir: None,
            command: Some(&cmd),
        };
        let line = build_invoke_line(&s);
        // The whole shell-injection-shaped argv element must be a
        // single shell word in the invoke line.
        assert!(line.contains(
            "'bash' '-ic' 'sudo -u dev bash -ic '\"'\"'cd app && assist'\"'\"''"
        ));
    }

    #[test]
    fn shell_quote_drops_nul_and_control_chars() {
        // NUL would truncate eval input on many shells.
        assert_eq!(shell_quote("ab\0cd"), "'abcd'");
        // Other ASCII controls would corrupt the invoke line.
        assert_eq!(shell_quote("a\x01b\x07c"), "'abc'");
        // \t is allowed (it's not a line-terminator).
        assert_eq!(shell_quote("a\tb"), "'a\tb'");
        // Newlines stripped.
        assert_eq!(shell_quote("a\nb"), "'ab'");
    }

    #[test]
    fn build_invoke_empty_command_omits_dash_dash() {
        let cmd: Vec<String> = vec![];
        let s = EnterSignal {
            workspace: "foo",
            dir: None,
            command: Some(&cmd),
        };
        // --new is always present; the bare `-- ` command-separator
        // should NOT be appended when the argv is empty.
        let line = build_invoke_line(&s);
        assert!(!line.contains("-- '"), "got: {line}");
        assert!(line.ends_with("'foo'"));
    }
}
