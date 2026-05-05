use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct IdleTtl(Duration);

impl IdleTtl {
    pub fn parse(input: &str) -> Result<Self, IdleTtlParseError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(IdleTtlParseError::Empty);
        }
        if input == "0" {
            return Ok(Self(Duration::ZERO));
        }

        let (number, unit) = split_duration(input)?;
        let value = number
            .parse::<u64>()
            .map_err(|_| IdleTtlParseError::InvalidNumber(number.to_string()))?;

        let seconds = match unit {
            "s" | "sec" | "secs" | "second" | "seconds" => value,
            "m" | "min" | "mins" | "minute" | "minutes" => value.saturating_mul(60),
            "h" | "hr" | "hrs" | "hour" | "hours" => value.saturating_mul(60 * 60),
            "d" | "day" | "days" => value.saturating_mul(24 * 60 * 60),
            _ => return Err(IdleTtlParseError::InvalidUnit(unit.to_string())),
        };

        Ok(Self(Duration::from_secs(seconds)))
    }

    pub fn from_duration(duration: Duration) -> Self {
        Self(duration)
    }

    pub fn as_duration(self) -> Duration {
        self.0
    }

    pub fn expires_at(self, last_active_at: SystemTime) -> Option<SystemTime> {
        if self.0.is_zero() {
            return None;
        }
        last_active_at.checked_add(self.0)
    }

    pub fn is_expired_at(self, last_active_at: SystemTime, now: SystemTime) -> bool {
        self.expires_at(last_active_at)
            .map(|expires_at| now >= expires_at)
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdleTtlParseError {
    #[error("idle TTL cannot be empty")]
    Empty,
    #[error("idle TTL must include a number before the unit")]
    MissingNumber,
    #[error("idle TTL must include a unit")]
    MissingUnit,
    #[error("invalid idle TTL number: {0}")]
    InvalidNumber(String),
    #[error("invalid idle TTL unit: {0}")]
    InvalidUnit(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLifecycleState {
    pub status: RuntimeStatus,
    pub idle_ttl: Option<IdleTtl>,
    pub started_at: SystemTime,
    pub last_active_at: SystemTime,
    pub stopped_at: Option<SystemTime>,
}

impl RuntimeLifecycleState {
    pub fn started(now: SystemTime, idle_ttl: Option<IdleTtl>) -> Self {
        Self {
            status: RuntimeStatus::Running,
            idle_ttl,
            started_at: now,
            last_active_at: now,
            stopped_at: None,
        }
    }

    pub fn mark_active(&mut self, at: SystemTime) {
        self.last_active_at = at;
        if self.status == RuntimeStatus::Idle {
            self.status = RuntimeStatus::Running;
        }
    }

    pub fn mark_idle(&mut self, at: SystemTime) {
        self.last_active_at = at;
        if self.status == RuntimeStatus::Running {
            self.status = RuntimeStatus::Idle;
        }
    }

    pub fn stop(&mut self, at: SystemTime, reason: StopReason) {
        self.status = RuntimeStatus::Stopped(reason);
        self.stopped_at = Some(at);
    }

    pub fn should_stop_for_idle_ttl(&self, now: SystemTime) -> bool {
        matches!(self.status, RuntimeStatus::Idle)
            && self
                .idle_ttl
                .map(|ttl| ttl.is_expired_at(self.last_active_at, now))
                .unwrap_or(false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeStatus {
    Running,
    Idle,
    Stopped(StopReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    UserRequested,
    IdleTtlExpired,
    RuntimeExited,
}

fn split_duration(input: &str) -> Result<(&str, &str), IdleTtlParseError> {
    let unit_start = input
        .char_indices()
        .find_map(|(idx, ch)| (!ch.is_ascii_digit()).then_some(idx));

    match unit_start {
        Some(0) => Err(IdleTtlParseError::MissingNumber),
        Some(idx) => Ok((&input[..idx], input[idx..].trim())),
        None => Err(IdleTtlParseError::MissingUnit),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_idle_ttl_units() {
        assert_eq!(
            IdleTtl::parse("30s").unwrap().as_duration(),
            Duration::from_secs(30)
        );
        assert_eq!(
            IdleTtl::parse("15m").unwrap().as_duration(),
            Duration::from_secs(900)
        );
        assert_eq!(
            IdleTtl::parse("2h").unwrap().as_duration(),
            Duration::from_secs(7200)
        );
        assert_eq!(
            IdleTtl::parse("1day").unwrap().as_duration(),
            Duration::from_secs(86400)
        );
    }

    #[test]
    fn zero_idle_ttl_disables_expiration() {
        let ttl = IdleTtl::parse("0").unwrap();
        let now = SystemTime::UNIX_EPOCH;

        assert_eq!(ttl.expires_at(now), None);
        assert!(!ttl.is_expired_at(now, now + Duration::from_secs(999)));
    }

    #[test]
    fn rejects_invalid_idle_ttl() {
        assert_eq!(IdleTtl::parse(""), Err(IdleTtlParseError::Empty));
        assert_eq!(IdleTtl::parse("15"), Err(IdleTtlParseError::MissingUnit));
        assert_eq!(IdleTtl::parse("m"), Err(IdleTtlParseError::MissingNumber));
        assert_eq!(
            IdleTtl::parse("15fortnights"),
            Err(IdleTtlParseError::InvalidUnit("fortnights".to_string()))
        );
    }

    #[test]
    fn lifecycle_state_reports_idle_expiration() {
        let started_at = SystemTime::UNIX_EPOCH;
        let mut state = RuntimeLifecycleState::started(
            started_at,
            Some(IdleTtl::from_duration(Duration::from_secs(60))),
        );

        state.mark_idle(started_at + Duration::from_secs(10));

        assert!(!state.should_stop_for_idle_ttl(started_at + Duration::from_secs(69)));
        assert!(state.should_stop_for_idle_ttl(started_at + Duration::from_secs(70)));
    }
}
