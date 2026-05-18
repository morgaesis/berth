use super::protocol::{read_frame, write_frame, Frame};
use anyhow::{Context, Result};
use std::os::unix::io::AsRawFd;
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

pub async fn attach<P: AsRef<Path>>(socket_path: P) -> Result<i32> {
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

    let (cols, rows) = current_size(stdout_fd);
    write_frame(&mut write_half, &Frame::Resize { cols, rows })
        .await
        .ok();

    let stdin_task = tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if write_frame(&mut write_half, &Frame::Stdin(buf[..n].to_vec()))
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
