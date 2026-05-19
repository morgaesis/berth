use super::protocol::{read_frame, write_frame, Frame};
use anyhow::{Context, Result};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// Error returned by `attach` when another client already holds the
/// session lock. Callers in the `--resume-or-new` path treat this as a
/// signal to try the next session or spawn a new one; explicit `attach`
/// surfaces it to the user.
#[derive(Debug)]
pub struct SessionBusy;

impl std::fmt::Display for SessionBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "session is already attached by another client; pass --new to start a fresh one"
        )
    }
}

impl std::error::Error for SessionBusy {}

/// Companion lock-file path for a session socket.
fn lock_path_for(socket_path: &Path) -> PathBuf {
    let mut p = socket_path.to_path_buf();
    let new_ext = match p.extension() {
        Some(ext) => format!("{}.client-lock", ext.to_string_lossy()),
        None => "client-lock".to_string(),
    };
    p.set_extension(new_ext);
    p
}

/// Probe whether a session is currently attached, without disturbing
/// it. Returns true if no client holds the flock; false if held.
/// Used by `attach --resume-or-new` to skip busy sessions.
pub fn is_session_free(socket_path: &Path) -> bool {
    use nix::fcntl::{Flock, FlockArg};
    let lock_path = lock_path_for(socket_path);
    let f = match std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
    {
        Ok(f) => f,
        Err(_) => return true, // can't even open the lock file; assume free
    };
    // `Flock::lock` consumes the file and returns an RAII handle; when
    // it drops at the end of this scope, the lock is released. So just
    // attempting the lock is the probe.
    match Flock::lock(f, FlockArg::LockExclusiveNonblock) {
        Ok(_lock) => true,
        Err(_) => false,
    }
}

pub async fn attach<P: AsRef<Path>>(socket_path: P) -> Result<i32> {
    // Take an exclusive flock on a sibling lock file. The lock handle
    // is leaked so it stays alive for the lifetime of this process;
    // the kernel releases the lock automatically when the process dies
    // (including via SSH-drop / hibernation), so any subsequent
    // `resume-or-new` will find this session free again.
    use nix::fcntl::{Flock, FlockArg};
    let lock_path = lock_path_for(socket_path.as_ref());
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("opening session lock {}", lock_path.display()))?;
    match Flock::lock(lock_file, FlockArg::LockExclusiveNonblock) {
        Ok(lock) => {
            // Leak into a heap allocation so it stays alive (and the
            // kernel keeps holding the flock) until process exit.
            Box::leak(Box::new(lock));
        }
        Err((_, nix::errno::Errno::EWOULDBLOCK)) => return Err(SessionBusy.into()),
        Err((_, e)) => return Err(anyhow::Error::new(e).context("acquiring session lock")),
    }

    let stream = UnixStream::connect(&socket_path).await.with_context(|| {
        format!(
            "connecting to session socket {}",
            socket_path.as_ref().display()
        )
    })?;
    let (mut read_half, mut write_half) = stream.into_split();

    let stdin_fd = std::io::stdin().as_raw_fd();
    let stdout_fd = std::io::stdout().as_raw_fd();
    let raw_guard = RawTtyGuard::new(stdin_fd)?;

    // Multiplex outgoing frames (initial resize + stdin chunks + SIGWINCH
    // resize updates) through one mpsc → one writer task. Without this
    // serialization the stdin task and a SIGWINCH task would both want
    // exclusive access to write_half.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Frame>(64);

    // Initial size handshake.
    let (cols, rows) = current_size(stdout_fd);
    out_tx.send(Frame::Resize { cols, rows }).await.ok();

    let writer_task = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            if write_frame(&mut write_half, &frame).await.is_err() {
                break;
            }
        }
    });

    // SIGWINCH propagation: on every terminal resize, query the new
    // size and send a Resize frame. Without this, claude / vim / less
    // never re-wrap when the user resizes their window.
    let resize_tx = out_tx.clone();
    let winch_task = tokio::spawn(async move {
        let mut signal =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change()) {
                Ok(s) => s,
                Err(_) => return,
            };
        while signal.recv().await.is_some() {
            let (cols, rows) = current_size(stdout_fd);
            if resize_tx.send(Frame::Resize { cols, rows }).await.is_err() {
                break;
            }
        }
    });

    let stdin_tx = out_tx.clone();
    let stdin_task = tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if stdin_tx
                        .send(Frame::Stdin(buf[..n].to_vec()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        Ok::<_, anyhow::Error>(())
    });
    drop(out_tx); // sender count = stdin_tx + resize_tx now

    let mut exit_code: i32 = 0;
    let mut stdout = tokio::io::stdout();
    loop {
        match read_frame(&mut read_half).await {
            Ok(Some(Frame::Stdout(bytes))) => {
                if stdout.write_all(&bytes).await.is_err() {
                    break;
                }
                let _ = stdout.flush().await;
            }
            Ok(Some(Frame::Exit { code })) => {
                exit_code = code;
                break;
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => break,
        }
    }
    drop(raw_guard);
    stdin_task.abort();
    winch_task.abort();
    writer_task.abort();
    // Drain stdout so the last bytes (commonly a shell prompt or exit
    // banner) actually reach the user's terminal before we exit.
    let _ = stdout.flush().await;

    // `tokio::io::stdin()` reads via a dedicated blocking thread; aborting
    // the future cancels the await but the read syscall on the underlying
    // FD is still pending. On Linux that keeps fd 0 open in this process,
    // which keeps the SSH PTY open, which keeps the user's shell hanging
    // until they hit a key to satisfy the read. Force-terminate the
    // process so the kernel cleans up our descriptors and SSH disconnects
    // immediately. raw_guard was dropped above so the user's tty is back
    // in cooked mode by the time the process actually goes away.
    std::process::exit(exit_code)
}

fn current_size(fd: i32) -> (u16, u16) {
    let mut ws = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe {
        if libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) == 0 {
            return (ws.ws_col, ws.ws_row);
        }
    }
    (80, 24)
}

struct RawTtyGuard {
    fd: i32,
    saved: Option<libc::termios>,
}

impl RawTtyGuard {
    fn new(fd: i32) -> Result<Self> {
        let mut termios: libc::termios = unsafe { std::mem::zeroed() };
        let saved = unsafe {
            if libc::tcgetattr(fd, &mut termios) != 0 {
                return Ok(Self { fd, saved: None });
            }
            let original = termios;
            libc::cfmakeraw(&mut termios);
            libc::tcsetattr(fd, libc::TCSANOW, &termios);
            Some(original)
        };
        Ok(Self { fd, saved })
    }
}

impl Drop for RawTtyGuard {
    fn drop(&mut self) {
        if let Some(saved) = self.saved.take() {
            unsafe {
                libc::tcsetattr(self.fd, libc::TCSANOW, &saved);
            }
        }
    }
}
