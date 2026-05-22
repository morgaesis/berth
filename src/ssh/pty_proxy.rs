//! Run `ssh` under a PTY we control, filter OSC 7 out of its output,
//! forward stdin/stdout/SIGWINCH back and forth.
//!
//! The user's terminal still sees a fully-interactive ssh session
//! (raw mode, signals, resize), but the remote shell's
//! cwd-via-OSC-7 emissions are stripped before they reach the local
//! emulator. berth's own marker-dir OSC 7 — emitted *before* the
//! ssh child starts — stays the authoritative cwd for new tabs.
//!
//! Why a wrapper and not a remote-side fix: the request was an
//! explicit no-remote-interference. The wrapper is a single,
//! local, well-bounded transform; once it's correct, the remote
//! shell's behaviour is irrelevant.

use super::osc7_filter::Osc7Filter;
use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::sync::mpsc;
use std::thread;

/// Run `ssh <args>` in a local PTY pair; return the ssh exit code.
/// OSC 7 sequences in the ssh stdout stream are stripped before
/// reaching the user's terminal.
pub fn ssh_through_filter(args: &[String]) -> Result<i32> {
    let stdin_fd = std::io::stdin().as_raw_fd();
    let stdout_fd = std::io::stdout().as_raw_fd();
    let (cols, rows) = current_terminal_size(stdout_fd);

    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("opening pty pair for ssh wrapper")?;

    let mut cmd = CommandBuilder::new("ssh");
    for arg in args {
        cmd.arg(arg);
    }
    // Pass the local environment through. CommandBuilder defaults to
    // an empty env on spawn; without this the ssh child would lose
    // SSH_AUTH_SOCK, the user's $HOME, etc., and pubkey auth would
    // start failing in subtle ways.
    for (k, v) in std::env::vars() {
        cmd.env(k, v);
    }

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .context("spawning ssh under the local PTY")?;
    drop(pair.slave);

    // Put the local terminal into raw mode so keystrokes flow through
    // to ssh unmodified (Ctrl-C, etc. become bytes the remote shell
    // sees rather than signals delivered to ssh's controlling tty).
    let _raw_guard = RawTtyGuard::install(stdin_fd)?;

    let mut master_reader = pair
        .master
        .try_clone_reader()
        .context("cloning pty master reader")?;
    let mut master_writer = pair
        .master
        .take_writer()
        .context("taking pty master writer")?;

    // SIGWINCH propagation: the local terminal's controlling tty
    // dispatches resize signals to *this* process; we have to
    // forward them through to the inner PTY so the remote shell
    // sees them (and SSH transmits to its remote-side TTY peer).
    //
    // portable-pty's `master.resize()` is sync; we run the
    // sigwinch handler on its own thread and call resize from there.
    let master_for_resize = pair.master;
    let (winch_tx, winch_rx) = mpsc::channel::<PtySize>();
    let winch_thread = thread::spawn(move || {
        use signal_hook::consts::SIGWINCH;
        use signal_hook::iterator::Signals;
        let mut signals = match Signals::new([SIGWINCH]) {
            Ok(s) => s,
            Err(_) => return,
        };
        for _ in &mut signals {
            let (cols, rows) = current_terminal_size(stdout_fd);
            let sz = PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            };
            if winch_tx.send(sz).is_err() {
                break;
            }
        }
    });
    let resize_thread = thread::spawn(move || {
        while let Ok(sz) = winch_rx.recv() {
            let _ = master_for_resize.resize(sz);
        }
    });

    // stdin -> pty.master
    let stdin_thread = thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if master_writer.write_all(&buf[..n]).is_err() {
                        break;
                    }
                    let _ = master_writer.flush();
                }
                Err(_) => break,
            }
        }
    });

    // pty.master -> OSC 7 filter -> stdout
    let reader_thread = thread::spawn(move || {
        let stdout = std::io::stdout();
        let mut filter = Osc7Filter::new(stdout.lock());
        let mut buf = [0u8; 4096];
        loop {
            match master_reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if filter.filter(&buf[..n]).is_err() {
                        break;
                    }
                    let _ = filter.flush();
                }
                Err(_) => break,
            }
        }
    });

    // Wait for ssh to exit.
    let status = child.wait().context("waiting for ssh child")?;
    let exit_code = status.exit_code() as i32;

    // Reader thread will exit when the PTY master sees EOF on the
    // slave side (which happens when ssh exits). Stdin thread is
    // best-effort: a pending blocking read on fd 0 keeps it alive
    // until the user hits a key, so we don't wait for it.
    drop(reader_thread);
    drop(stdin_thread);
    drop(winch_thread);
    drop(resize_thread);

    Ok(exit_code)
}

fn current_terminal_size(stdout_fd: i32) -> (u16, u16) {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::ioctl(stdout_fd, libc::TIOCGWINSZ, &mut size as *mut _) };
    if rc == 0 && size.ws_col > 0 && size.ws_row > 0 {
        (size.ws_col, size.ws_row)
    } else {
        // Sensible defaults so resize-emitter still sends something
        // useful to the remote.
        (80, 24)
    }
}

/// Put the controlling tty into raw mode on construction and restore
/// it on drop. Same shape as the supervisor's RawTtyGuard but local
/// to this module (we don't want to depend on session client wiring
/// from inside the ssh module).
struct RawTtyGuard {
    fd: i32,
    original: nix::sys::termios::Termios,
}

impl RawTtyGuard {
    fn install(fd: i32) -> Result<Self> {
        use nix::sys::termios::{tcgetattr, tcsetattr, LocalFlags, SetArg};
        // Skip if stdin isn't actually a tty (running under tests, in
        // a pipe, etc.) — `tcgetattr` returns ENOTTY and we'd otherwise
        // refuse to spawn ssh at all.
        let original = match tcgetattr(unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) }) {
            Ok(t) => t,
            Err(_) => return Ok(Self::noop()),
        };
        let mut raw = original.clone();
        // Mirror cfmakeraw() for the bits we care about: turn off
        // canonical input, echo, signal generation. The remote tty
        // takes over those responsibilities.
        raw.local_flags
            .remove(LocalFlags::ICANON | LocalFlags::ECHO | LocalFlags::ISIG | LocalFlags::IEXTEN);
        raw.input_flags.remove(
            nix::sys::termios::InputFlags::IXON
                | nix::sys::termios::InputFlags::ICRNL
                | nix::sys::termios::InputFlags::INPCK
                | nix::sys::termios::InputFlags::ISTRIP,
        );
        raw.output_flags
            .remove(nix::sys::termios::OutputFlags::OPOST);
        tcsetattr(
            unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) },
            SetArg::TCSANOW,
            &raw,
        )
        .context("setting tty raw mode")?;
        Ok(Self { fd, original })
    }

    fn noop() -> Self {
        // Build a dummy termios. We only restore it under the matching
        // `if self.fd >= 0` check below, so the value never gets used.
        use nix::sys::termios::Termios;
        let t: Termios = unsafe { std::mem::zeroed() };
        Self {
            fd: -1,
            original: t,
        }
    }
}

impl Drop for RawTtyGuard {
    fn drop(&mut self) {
        if self.fd < 0 {
            return;
        }
        use nix::sys::termios::{tcsetattr, SetArg};
        let _ = tcsetattr(
            unsafe { std::os::fd::BorrowedFd::borrow_raw(self.fd) },
            SetArg::TCSANOW,
            &self.original,
        );
    }
}
