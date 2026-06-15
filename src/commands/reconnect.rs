use anyhow::Result;
use std::env;
use std::io::IsTerminal;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

const DEFAULT_BACKOFF_START_MS: u64 = 500;
const DEFAULT_BACKOFF_CAP_MS: u64 = 5 * 60 * 1000;
const KEY_POLL_SLICE: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconnectWake {
    TimedOut,
    KeyPressed,
}

#[derive(Debug, Clone, Copy)]
pub struct ReconnectBackoff {
    start_ms: u64,
    cap_ms: u64,
    current_ms: u64,
}

impl ReconnectBackoff {
    pub fn from_env() -> Self {
        let (start_ms, cap_ms) = reconnect_backoff_params();
        Self {
            start_ms,
            cap_ms,
            current_ms: start_ms,
        }
    }

    pub fn current_ms(&self) -> u64 {
        self.current_ms
    }

    pub fn reset(&mut self) {
        self.current_ms = self.start_ms;
    }

    pub async fn wait_and_advance(&mut self) -> Result<ReconnectWake> {
        let delay = Duration::from_millis(self.current_ms);
        let wake = wait_for_reconnect_retry(delay).await?;
        self.current_ms = self.current_ms.saturating_mul(2).min(self.cap_ms);
        Ok(wake)
    }
}

/// Initial and capped backoff (ms) for remote reconnect/session-recovery
/// loops. Defaults to 500ms -> 5m, overridable via
/// `BERTH_RECONNECT_BACKOFF_MS` and `BERTH_RECONNECT_BACKOFF_CAP_MS`.
/// The cap is floored to the start so a misconfiguration cannot invert them.
pub fn reconnect_backoff_params() -> (u64, u64) {
    let start = env::var("BERTH_RECONNECT_BACKOFF_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_BACKOFF_START_MS);
    let cap = env::var("BERTH_RECONNECT_BACKOFF_CAP_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_BACKOFF_CAP_MS)
        .max(start);
    (start, cap)
}

pub async fn wait_for_reconnect_retry(delay: Duration) -> Result<ReconnectWake> {
    if !std::io::stdin().is_terminal() || keypress_wait_disabled() {
        tokio::time::sleep(delay).await;
        return Ok(ReconnectWake::TimedOut);
    }

    let cancelled = Arc::new(AtomicBool::new(false));
    let wait_cancelled = cancelled.clone();
    let mut handle =
        tokio::task::spawn_blocking(move || wait_for_keypress_or_timeout(delay, wait_cancelled));

    tokio::select! {
        result = &mut handle => result
            .map_err(|err| anyhow::anyhow!("reconnect wait task failed: {err}"))?,
        _ = tokio::signal::ctrl_c() => {
            cancelled.store(true, Ordering::Release);
            let _ = handle.await;
            Err(anyhow::anyhow!("aborted by Ctrl+C"))
        }
    }
}

fn keypress_wait_disabled() -> bool {
    matches!(
        env::var("BERTH_RECONNECT_KEY_WAKE").as_deref(),
        Ok("0") | Ok("false") | Ok("FALSE") | Ok("off") | Ok("OFF")
    )
}

#[cfg(unix)]
fn wait_for_keypress_or_timeout(
    delay: Duration,
    cancelled: Arc<AtomicBool>,
) -> Result<ReconnectWake> {
    use std::os::unix::io::AsRawFd;

    let stdin = std::io::stdin();
    let fd = stdin.as_raw_fd();
    let _guard = CbreakNoEchoGuard::new(fd)?;
    let deadline = Instant::now() + delay;
    let mut buf = [0u8; 64];

    loop {
        if cancelled.load(Ordering::Acquire) {
            return Ok(ReconnectWake::TimedOut);
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Ok(ReconnectWake::TimedOut);
        };
        let timeout = remaining.min(KEY_POLL_SLICE);
        let mut fds = [libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        }];
        let rc =
            unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, millis(timeout)) };
        if rc == 0 {
            continue;
        }
        if rc < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err.into());
        }
        if fds[0].revents & libc::POLLIN != 0 {
            let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n > 0 {
                return Ok(ReconnectWake::KeyPressed);
            }
            if n == 0 {
                return Ok(ReconnectWake::TimedOut);
            }
            let err = std::io::Error::last_os_error();
            if err.kind() != std::io::ErrorKind::Interrupted {
                return Err(err.into());
            }
        }
        if fds[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Ok(ReconnectWake::TimedOut);
        }
    }
}

#[cfg(unix)]
struct CbreakNoEchoGuard {
    fd: i32,
    original: libc::termios,
}

#[cfg(unix)]
impl CbreakNoEchoGuard {
    fn new(fd: i32) -> Result<Self> {
        let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
        if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let original = unsafe { original.assume_init() };
        let mut cbreak = original;
        cbreak.c_lflag &= !(libc::ICANON | libc::ECHO);
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &cbreak) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Self { fd, original })
    }
}

#[cfg(unix)]
impl Drop for CbreakNoEchoGuard {
    fn drop(&mut self) {
        let _ = unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.original) };
    }
}

#[cfg(windows)]
fn wait_for_keypress_or_timeout(
    delay: Duration,
    cancelled: Arc<AtomicBool>,
) -> Result<ReconnectWake> {
    use crossterm::event::{self, Event, KeyCode, KeyModifiers};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

    struct RawModeGuard;
    impl Drop for RawModeGuard {
        fn drop(&mut self) {
            let _ = disable_raw_mode();
        }
    }

    enable_raw_mode()?;
    let _guard = RawModeGuard;
    let deadline = Instant::now() + delay;

    loop {
        if cancelled.load(Ordering::Acquire) {
            return Ok(ReconnectWake::TimedOut);
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Ok(ReconnectWake::TimedOut);
        };
        if !event::poll(remaining.min(KEY_POLL_SLICE))? {
            continue;
        }
        if let Event::Key(key) = event::read()? {
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return Err(anyhow::anyhow!("aborted by Ctrl+C"));
            }
            return Ok(ReconnectWake::KeyPressed);
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn wait_for_keypress_or_timeout(
    delay: Duration,
    cancelled: Arc<AtomicBool>,
) -> Result<ReconnectWake> {
    let deadline = Instant::now() + delay;
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Ok(ReconnectWake::TimedOut);
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Ok(ReconnectWake::TimedOut);
        };
        std::thread::sleep(remaining.min(KEY_POLL_SLICE));
    }
}

#[cfg(unix)]
fn millis(duration: Duration) -> i32 {
    duration.as_millis().min(i32::MAX as u128) as i32
}

#[cfg(test)]
mod tests {
    use super::{reconnect_backoff_params, ReconnectBackoff};
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn reconnect_backoff_params_default_to_five_minute_cap() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("BERTH_RECONNECT_BACKOFF_MS");
        std::env::remove_var("BERTH_RECONNECT_BACKOFF_CAP_MS");

        assert_eq!(reconnect_backoff_params(), (500, 300_000));
    }

    #[test]
    fn reconnect_backoff_params_ignore_invalid_values_and_floor_cap() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("BERTH_RECONNECT_BACKOFF_MS", "2000");
        std::env::set_var("BERTH_RECONNECT_BACKOFF_CAP_MS", "1000");
        assert_eq!(reconnect_backoff_params(), (2000, 2000));

        std::env::set_var("BERTH_RECONNECT_BACKOFF_MS", "0");
        std::env::set_var("BERTH_RECONNECT_BACKOFF_CAP_MS", "not-a-number");
        assert_eq!(reconnect_backoff_params(), (500, 300_000));

        std::env::remove_var("BERTH_RECONNECT_BACKOFF_MS");
        std::env::remove_var("BERTH_RECONNECT_BACKOFF_CAP_MS");
    }

    #[test]
    fn reconnect_backoff_doubles_until_cap_and_resets() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("BERTH_RECONNECT_BACKOFF_MS", "5");
        std::env::set_var("BERTH_RECONNECT_BACKOFF_CAP_MS", "12");

        let mut backoff = ReconnectBackoff::from_env();
        assert_eq!(backoff.current_ms(), 5);
        backoff.current_ms = backoff.current_ms.saturating_mul(2).min(backoff.cap_ms);
        assert_eq!(backoff.current_ms(), 10);
        backoff.current_ms = backoff.current_ms.saturating_mul(2).min(backoff.cap_ms);
        assert_eq!(backoff.current_ms(), 12);
        backoff.reset();
        assert_eq!(backoff.current_ms(), 5);

        std::env::remove_var("BERTH_RECONNECT_BACKOFF_MS");
        std::env::remove_var("BERTH_RECONNECT_BACKOFF_CAP_MS");
    }
}
