use super::protocol::{read_frame, write_frame, Frame};
use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, Mutex};

pub struct SupervisorConfig {
    pub socket_path: PathBuf,
    pub workspace: String,
    pub command: Vec<String>,
    pub workdir: Option<PathBuf>,
    pub initial_size: PtySize,
}

pub async fn run(config: SupervisorConfig) -> Result<i32> {
    if let Some(parent) = config.socket_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating session socket dir {}", parent.display()))?;
    }
    if config.socket_path.exists() {
        let _ = fs::remove_file(&config.socket_path);
    }

    // Bind under a restrictive umask so the socket is created with mode 0600
    // atomically; without this, the kernel applies the inherited umask (often
    // 0022) and there's a TOCTOU window between bind and the explicit chmod.
    let prev_umask = unsafe { libc::umask(0o077) };
    let listener_result = UnixListener::bind(&config.socket_path);
    unsafe {
        libc::umask(prev_umask);
    }
    let listener = listener_result
        .with_context(|| format!("binding session socket {}", config.socket_path.display()))?;
    set_socket_perms(&config.socket_path)?;

    let pty = native_pty_system();
    let pair = pty
        .openpty(config.initial_size)
        .context("opening PTY pair")?;

    let mut cmd = if config.command.is_empty() {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let mut c = CommandBuilder::new(shell);
        c.arg("-l");
        c
    } else {
        let mut c = CommandBuilder::new(&config.command[0]);
        for arg in &config.command[1..] {
            c.arg(arg);
        }
        c
    };
    // portable-pty's CommandBuilder does NOT inherit the parent process's
    // cwd when no `cwd()` is set — it defaults to `$HOME`. The SSH cascade
    // does `cd $remote_dir` before exec'ing us, which sets the supervisor
    // process's cwd; we have to copy that onto the CommandBuilder
    // explicitly or the supervised command (claude, bash, …) lands in
    // /home/<user> instead of the workspace dir.
    let effective_cwd = config
        .workdir
        .clone()
        .or_else(|| std::env::current_dir().ok());
    if let Some(dir) = &effective_cwd {
        cmd.cwd(dir);
    }
    cmd.env("BERTH_WORKSPACE", &config.workspace);

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .context("spawning workspace shell under PTY")?;
    drop(pair.slave);

    let pty_master = Arc::new(Mutex::new(pair.master));

    // Sticky shutdown signal. handle_client tasks spawned during the
    // grace period miss the original broadcast (broadcasts don't replay
    // past messages), so each task ALSO checks this on entry: if set,
    // we're already shutting down → send replay+Exit and return
    // without taking ownership of stdin_tx/resize_tx clones for the
    // long haul, which would otherwise hang writer_handle's cleanup.
    let shutdown_fired: Arc<std::sync::atomic::AtomicI32> =
        Arc::new(std::sync::atomic::AtomicI32::new(i32::MIN));

    let (stdout_tx, _) = broadcast::channel::<Vec<u8>>(64);
    let (stdin_tx, stdin_rx) = mpsc::channel::<Vec<u8>>(64);
    let (resize_tx, mut resize_rx) = mpsc::channel::<(u16, u16)>(16);
    let (shutdown_tx, _) = broadcast::channel::<i32>(4);

    // Rolling replay buffer of the last 64 KiB of PTY output. Late
    // clients (anyone who connects after the child has already emitted
    // bytes — including the entire output of fast-exit commands like
    // `bash --help`) get this dumped at attach time before live
    // streaming starts.
    const REPLAY_CAP: usize = 64 * 1024;
    let replay_buf: Arc<std::sync::Mutex<Vec<u8>>> =
        Arc::new(std::sync::Mutex::new(Vec::with_capacity(REPLAY_CAP)));

    let stdout_for_pump = stdout_tx.clone();
    let replay_for_reader = replay_buf.clone();
    let pty_for_reader = pty_master.clone();
    let shutdown_for_reader = shutdown_tx.clone();
    let pty_reader_handle = tokio::task::spawn_blocking(move || {
        let mut reader = match pty_for_reader.blocking_lock().try_clone_reader() {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(?e, "could not clone PTY reader");
                return;
            }
        };
        let mut buf = [0u8; 8192];
        loop {
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = buf[..n].to_vec();
                    // Append to replay buffer, dropping from the front
                    // when we exceed the cap. Cheap because mostly the
                    // buffer won't grow past cap.
                    if let Ok(mut b) = replay_for_reader.lock() {
                        b.extend_from_slice(&chunk);
                        if b.len() > REPLAY_CAP {
                            let overflow = b.len() - REPLAY_CAP;
                            b.drain(..overflow);
                        }
                    }
                    let _ = stdout_for_pump.send(chunk);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        let _ = shutdown_for_reader.send(0);
    });

    let writer = pty_master
        .lock()
        .await
        .take_writer()
        .context("taking PTY writer")?;
    let mut writer = writer;
    let writer_handle = tokio::task::spawn_blocking(move || {
        let mut rx = stdin_rx;
        while let Some(bytes) = rx.blocking_recv() {
            if writer.write_all(&bytes).is_err() {
                break;
            }
            let _ = writer.flush();
        }
    });

    let pty_for_resize = pty_master.clone();
    let resizer_handle = tokio::spawn(async move {
        while let Some((cols, rows)) = resize_rx.recv().await {
            let master = pty_for_resize.lock().await;
            let _ = master.resize(PtySize {
                cols,
                rows,
                ..PtySize::default()
            });
        }
    });

    let socket_for_cleanup = config.socket_path.clone();
    let shutdown_for_waiter = shutdown_tx.clone();
    let waiter_handle = tokio::task::spawn_blocking(move || {
        let status = child.wait().ok();
        let code = status.as_ref().map(|s| s.exit_code() as i32).unwrap_or(-1);
        let _ = shutdown_for_waiter.send(code);
        code
    });

    let mut shutdown_rx = shutdown_tx.subscribe();
    let active_client = Arc::new(Mutex::new(0u64));
    let mut next_client_id = 1u64;

    let mut pending_exit: Option<i32> = None;
    loop {
        tokio::select! {
            res = listener.accept() => {
                match res {
                    Ok((stream, _addr)) => {
                        let stream_id = next_client_id;
                        next_client_id += 1;
                        let stdout_rx = stdout_tx.subscribe();
                        let stdin_for_client = stdin_tx.clone();
                        let resize_for_client = resize_tx.clone();
                        let shutdown_for_client = shutdown_tx.clone();
                        let active = active_client.clone();
                        let replay = replay_buf.clone();
                        let shutdown_fired_for_client = shutdown_fired.clone();
                        tokio::spawn(handle_client(
                            stream_id,
                            stream,
                            stdout_rx,
                            stdin_for_client,
                            resize_for_client,
                            shutdown_for_client,
                            active,
                            replay,
                            shutdown_fired_for_client,
                        ));
                    }
                    Err(e) => {
                        tracing::error!(?e, "accept failed");
                        break;
                    }
                }
            }
            code = shutdown_rx.recv() => {
                let c = code.unwrap_or(0);
                pending_exit = Some(c);
                shutdown_fired.store(c, std::sync::atomic::Ordering::SeqCst);
                break;
            }
        }
    }

    // Grace window: after the child has exited, keep the socket open
    // for ~2s so a client that was racing to connect can still attach,
    // receive the replay buffer (the child's full output if it was
    // short-lived), and a clean Exit frame. Without this, fast-exit
    // commands like `bash --help` produce no visible output.
    let grace_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        tokio::select! {
            res = listener.accept() => {
                if let Ok((stream, _addr)) = res {
                    let stream_id = next_client_id;
                    next_client_id += 1;
                    let stdout_rx = stdout_tx.subscribe();
                    let stdin_for_client = stdin_tx.clone();
                    let resize_for_client = resize_tx.clone();
                    let shutdown_for_client = shutdown_tx.clone();
                    let active = active_client.clone();
                    let replay = replay_buf.clone();
                    let shutdown_fired_for_client = shutdown_fired.clone();
                    tokio::spawn(handle_client(
                        stream_id,
                        stream,
                        stdout_rx,
                        stdin_for_client,
                        resize_for_client,
                        shutdown_for_client,
                        active,
                        replay,
                        shutdown_fired_for_client,
                    ));
                }
            }
            _ = tokio::time::sleep_until(grace_deadline) => break,
        }
    }

    tracing::info!("cleanup: awaiting pty_reader_handle");
    let _ = pty_reader_handle.await;
    tracing::info!("cleanup: dropping stdin_tx");
    drop(stdin_tx);
    tracing::info!("cleanup: awaiting writer_handle");
    let _ = writer_handle.await;
    tracing::info!("cleanup: dropping resize_tx");
    drop(resize_tx);
    tracing::info!("cleanup: awaiting resizer_handle");
    let _ = resizer_handle.await;
    tracing::info!("cleanup: awaiting waiter_handle");
    let final_code = waiter_handle.await.unwrap_or(pending_exit.unwrap_or(0));
    tracing::info!(?final_code, "cleanup: removing socket");
    let _ = fs::remove_file(&socket_for_cleanup);
    tracing::info!("cleanup: done");
    Ok(final_code)
}

#[allow(clippy::too_many_arguments)]
async fn handle_client(
    _client_id: u64,
    stream: UnixStream,
    mut stdout_rx: broadcast::Receiver<Vec<u8>>,
    stdin_tx: mpsc::Sender<Vec<u8>>,
    resize_tx: mpsc::Sender<(u16, u16)>,
    shutdown_tx: broadcast::Sender<i32>,
    _active: Arc<Mutex<u64>>,
    replay: Arc<std::sync::Mutex<Vec<u8>>>,
    shutdown_fired: Arc<std::sync::atomic::AtomicI32>,
) {
    let mut shutdown_rx = shutdown_tx.subscribe();
    let mut shutdown_rx_for_read = shutdown_tx.subscribe();
    let (mut read_half, mut write_half) = stream.into_split();

    // Replay any output the child has already produced. This is what
    // makes `bash --help` (and any fast-exit command) actually deliver
    // its output: the supervisor buffered it before any client could
    // subscribe to the broadcast channel.
    let replay_snapshot: Vec<u8> = replay.lock().map(|b| b.clone()).unwrap_or_default();
    if !replay_snapshot.is_empty() {
        let _ = write_frame(&mut write_half, &Frame::Stdout(replay_snapshot)).await;
    }

    // Grace-period short-circuit: if the supervisor has already fired
    // shutdown by the time we got here, the original broadcast has
    // come and gone — subscribing now gives us a receiver that will
    // never fire. Send the buffered output + Exit + return, releasing
    // our stdin_tx/resize_tx clones immediately so the supervisor's
    // writer_handle.await can drain.
    let pending = shutdown_fired.load(std::sync::atomic::Ordering::SeqCst);
    if pending != i32::MIN {
        let _ = write_frame(&mut write_half, &Frame::Exit { code: pending }).await;
        return;
    }

    let writer_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                msg = stdout_rx.recv() => {
                    match msg {
                        Ok(bytes) => {
                            if write_frame(&mut write_half, &Frame::Stdout(bytes)).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    }
                }
                code = shutdown_rx.recv() => {
                    let code = code.unwrap_or(0);
                    let _ = write_frame(&mut write_half, &Frame::Exit { code }).await;
                    break;
                }
            }
        }
    });

    loop {
        tokio::select! {
            frame = read_frame(&mut read_half) => {
                match frame {
                    Ok(Some(Frame::Stdin(bytes))) => {
                        if stdin_tx.send(bytes).await.is_err() {
                            break;
                        }
                    }
                    Ok(Some(Frame::Resize { cols, rows })) => {
                        let _ = resize_tx.send((cols, rows)).await;
                    }
                    Ok(Some(_)) | Ok(None) => break,
                    Err(_) => break,
                }
            }
            // Crucial for supervisor cleanup: if the supervisor's main
            // loop has fired shutdown but the client hasn't closed its
            // socket yet (or is just slow to receive Frame::Exit),
            // break out anyway so this task's `stdin_tx` clone drops.
            // Without this break, writer_handle's mpsc receiver never
            // returns None and the supervisor hangs forever in its
            // cleanup `await`.
            _ = shutdown_rx_for_read.recv() => break,
        }
    }
    let _ = writer_task.await;
}

#[cfg(unix)]
fn set_socket_perms(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_socket_perms(_path: &Path) -> Result<()> {
    Ok(())
}

pub fn detach_from_terminal() -> Result<()> {
    use nix::unistd;
    unistd::setsid().ok();
    let dev_null = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")?;
    use std::os::unix::io::AsRawFd;
    let fd = dev_null.as_raw_fd();
    unsafe {
        libc::dup2(fd, 0);
        libc::dup2(fd, 1);
        libc::dup2(fd, 2);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::protocol::{read_frame, write_frame, Frame};

    #[tokio::test]
    async fn supervisor_round_trip_with_cat_subprocess() {
        let dir = tempdir();
        let socket = dir.join("test.sock");
        let cfg = SupervisorConfig {
            socket_path: socket.clone(),
            workspace: "test".into(),
            command: vec!["/bin/cat".into()],
            workdir: None,
            initial_size: PtySize {
                cols: 80,
                rows: 24,
                pixel_width: 0,
                pixel_height: 0,
            },
        };

        let supervisor_handle = tokio::spawn(async move { run(cfg).await });

        wait_for_socket(&socket).await;

        let mut client = UnixStream::connect(&socket).await.expect("connect");
        let (mut read_half, mut write_half) = client.split();

        write_frame(&mut write_half, &Frame::Stdin(b"hello\n".to_vec()))
            .await
            .expect("send stdin");

        let mut got = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline && !contains_subslice(&got, b"hello") {
            match tokio::time::timeout(
                std::time::Duration::from_millis(500),
                read_frame(&mut read_half),
            )
            .await
            {
                Ok(Ok(Some(Frame::Stdout(bytes)))) => got.extend(bytes),
                Ok(Ok(Some(_))) => {}
                Ok(Ok(None)) => break,
                Ok(Err(_)) => break,
                Err(_) => continue,
            }
        }
        assert!(
            contains_subslice(&got, b"hello"),
            "expected echo of 'hello' from cat, got {:?}",
            String::from_utf8_lossy(&got)
        );

        // Send EOF to make cat exit cleanly.
        write_frame(&mut write_half, &Frame::Stdin(vec![0x04]))
            .await
            .expect("send eof");
        drop(client);

        let res = tokio::time::timeout(std::time::Duration::from_secs(3), supervisor_handle)
            .await
            .expect("supervisor returned in time")
            .expect("supervisor join");
        assert!(res.is_ok(), "supervisor returned error: {:?}", res.err());
        assert!(!socket.exists(), "supervisor should remove its socket");
    }

    fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    async fn wait_for_socket(path: &std::path::Path) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if path.exists() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("supervisor never created socket {}", path.display());
    }

    fn tempdir() -> PathBuf {
        // Unix sockets cap at 108 chars total; the project's
        // .cache/session-tests path exceeds that when run from a
        // worktree (e.g. `.claude/worktrees/<name>/.cache/...`). Use
        // the short XDG runtime dir which is also where production
        // session sockets live.
        let base = std::env::var("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })))
            .join("berth-tests");
        std::fs::create_dir_all(&base).expect("base dir");
        let unique = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dir = base.join(unique);
        std::fs::create_dir_all(&dir).expect("test dir");
        dir
    }
}
