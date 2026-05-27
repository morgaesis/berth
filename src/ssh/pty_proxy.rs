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
use crate::config::Config;
use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::thread;

/// Run `ssh <args>` in a local PTY pair; return the ssh exit code.
/// OSC 7 sequences in the ssh stdout stream are stripped before
/// reaching the user's terminal.
pub fn ssh_through_filter(args: &[String]) -> Result<i32> {
    let detach_key = Config::load()?.detach_key_bytes()?;
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
    let mut child_killer = child.clone_killer();
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
    let mut signals = signal_hook::iterator::Signals::new([signal_hook::consts::SIGWINCH]).ok();
    let signals_handle = signals.as_ref().map(|s| s.handle());
    let winch_thread = thread::spawn(move || {
        let Some(mut signals) = signals.take() else {
            return;
        };
        for _ in signals.forever() {
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

    let helper_shutdown = Arc::new(AtomicBool::new(false));
    let forced_reconnect = Arc::new(AtomicBool::new(false));
    let detach_requested = Arc::new(AtomicBool::new(false));
    let watchdog_shutdown = helper_shutdown.clone();
    let watchdog_forced_reconnect = forced_reconnect.clone();
    let suspend_watchdog_thread = thread::spawn(move || {
        let mut last = std::time::SystemTime::now();
        while !watchdog_shutdown.load(Ordering::Acquire) {
            thread::sleep(std::time::Duration::from_millis(500));
            let now = std::time::SystemTime::now();
            let gap = now
                .duration_since(last)
                .unwrap_or_else(|_| std::time::Duration::from_secs(0));
            if gap > std::time::Duration::from_secs(10) {
                watchdog_forced_reconnect.store(true, Ordering::Release);
                let _ = child_killer.kill();
                break;
            }
            last = now;
        }
    });

    // stdin -> pty.master
    let stdin_shutdown_for_thread = helper_shutdown.clone();
    let stdin_detach_requested = detach_requested.clone();
    let stdin_thread = thread::spawn(move || {
        match forward_stdin_until_shutdown(
            stdin_fd,
            &mut master_writer,
            &stdin_shutdown_for_thread,
            detach_key.as_deref(),
        ) {
            Ok(StdinForwardOutcome::Detached) => {
                stdin_detach_requested.store(true, Ordering::Release);
            }
            Ok(StdinForwardOutcome::Closed) | Err(_) => {}
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
    while !detach_requested.load(Ordering::Acquire) {
        if let Some(status) = child.try_wait().context("polling ssh child")? {
            let exit_code = if forced_reconnect.load(Ordering::Acquire) {
                255
            } else {
                status.exit_code() as i32
            };
            return finish_ssh_proxy(
                exit_code,
                helper_shutdown,
                signals_handle,
                stdin_thread,
                suspend_watchdog_thread,
                reader_thread,
                winch_thread,
                resize_thread,
            );
        }
        thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    let exit_code = 0;

    finish_ssh_proxy(
        exit_code,
        helper_shutdown,
        signals_handle,
        stdin_thread,
        suspend_watchdog_thread,
        reader_thread,
        winch_thread,
        resize_thread,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish_ssh_proxy(
    exit_code: i32,
    helper_shutdown: Arc<AtomicBool>,
    signals_handle: Option<signal_hook::iterator::Handle>,
    stdin_thread: thread::JoinHandle<()>,
    suspend_watchdog_thread: thread::JoinHandle<()>,
    reader_thread: thread::JoinHandle<()>,
    winch_thread: thread::JoinHandle<()>,
    resize_thread: thread::JoinHandle<()>,
) -> Result<i32> {
    // Stop helper threads before returning. Leaving an old stdin
    // thread blocked on fd 0 across the reconnect loop creates
    // multiple readers racing for keystrokes.
    helper_shutdown.store(true, Ordering::Release);
    if let Some(handle) = signals_handle {
        handle.close();
    }

    let _ = stdin_thread.join();
    let _ = suspend_watchdog_thread.join();
    let _ = reader_thread.join();
    let _ = winch_thread.join();
    let _ = resize_thread.join();

    Ok(exit_code)
}

enum StdinForwardOutcome {
    Closed,
    Detached,
}

fn forward_stdin_until_shutdown<W: Write>(
    stdin_fd: RawFd,
    writer: &mut W,
    shutdown: &AtomicBool,
    detach_key: Option<&[u8]>,
) -> std::io::Result<StdinForwardOutcome> {
    let mut buf = [0u8; 4096];
    let mut matcher = DetachMatcher::new(detach_key);
    while !shutdown.load(Ordering::Acquire) {
        let mut fds = [libc::pollfd {
            fd: stdin_fd,
            events: libc::POLLIN,
            revents: 0,
        }];
        let rc = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 100) };
        if rc == 0 {
            continue;
        }
        if rc < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if fds[0].revents & libc::POLLIN != 0 {
            let n = unsafe { libc::read(stdin_fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n == 0 {
                break;
            }
            if n < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            let chunk = &buf[..n as usize];
            match matcher.filter(chunk) {
                DetachFilterResult::Forward(bytes) => {
                    if !bytes.is_empty() {
                        writer.write_all(&bytes)?;
                        writer.flush()?;
                    }
                }
                DetachFilterResult::Detached(bytes) => {
                    if !bytes.is_empty() {
                        writer.write_all(&bytes)?;
                        writer.flush()?;
                    }
                    return Ok(StdinForwardOutcome::Detached);
                }
            }
            continue;
        }
        if fds[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            break;
        }
    }
    Ok(StdinForwardOutcome::Closed)
}

enum DetachFilterResult {
    Forward(Vec<u8>),
    Detached(Vec<u8>),
}

struct DetachMatcher<'a> {
    key: Option<&'a [u8]>,
    pending: Vec<u8>,
}

impl<'a> DetachMatcher<'a> {
    fn new(key: Option<&'a [u8]>) -> Self {
        Self {
            key: key.filter(|k| !k.is_empty()),
            pending: Vec::new(),
        }
    }

    fn filter(&mut self, bytes: &[u8]) -> DetachFilterResult {
        let Some(key) = self.key else {
            return DetachFilterResult::Forward(bytes.to_vec());
        };
        let mut out = Vec::with_capacity(self.pending.len() + bytes.len());
        for &byte in bytes {
            self.pending.push(byte);
            if self.pending == key {
                return DetachFilterResult::Detached(out);
            }
            if key.starts_with(&self.pending) {
                continue;
            }
            out.extend_from_slice(&self.pending);
            self.pending.clear();
        }
        DetachFilterResult::Forward(out)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdin_forwarder_drains_pipe_before_hup() {
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let read_fd = fds[0];
        let write_fd = fds[1];
        let payload = b"typed-before-close";

        let written = unsafe { libc::write(write_fd, payload.as_ptr().cast(), payload.len()) };
        assert_eq!(written, payload.len() as isize);
        assert_eq!(unsafe { libc::close(write_fd) }, 0);

        let shutdown = AtomicBool::new(false);
        let mut out = Vec::new();
        forward_stdin_until_shutdown(read_fd, &mut out, &shutdown, None).expect("forward stdin");
        assert_eq!(unsafe { libc::close(read_fd) }, 0);

        assert_eq!(out, payload);
    }

    #[test]
    fn detach_matcher_strips_detach_key_and_keeps_prior_bytes() {
        let mut matcher = DetachMatcher::new(Some(&[0x1d]));
        match matcher.filter(b"abc\x1dignored") {
            DetachFilterResult::Detached(bytes) => assert_eq!(bytes, b"abc"),
            DetachFilterResult::Forward(_) => panic!("expected detach"),
        }
    }
}
