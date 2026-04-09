use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkErrorKind {
    Timeout,
    Connect,
    Request,
    HttpStatus(u16),
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkAction {
    Retry,
    TerminalRemoved,
    Terminal,
}

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(300),
            max_delay: Duration::from_secs(5),
        }
    }
}

pub fn classify_reqwest_error(err: &reqwest::Error) -> NetworkErrorKind {
    if err.is_timeout() {
        return NetworkErrorKind::Timeout;
    }
    if err.is_connect() {
        return NetworkErrorKind::Connect;
    }
    if err.is_request() {
        return NetworkErrorKind::Request;
    }
    if let Some(status) = err.status() {
        return NetworkErrorKind::HttpStatus(status.as_u16());
    }
    NetworkErrorKind::Unknown
}

pub fn action_for(kind: NetworkErrorKind) -> NetworkAction {
    match kind {
        NetworkErrorKind::HttpStatus(404) | NetworkErrorKind::HttpStatus(410) => {
            NetworkAction::TerminalRemoved
        }
        NetworkErrorKind::HttpStatus(408)
        | NetworkErrorKind::HttpStatus(425)
        | NetworkErrorKind::HttpStatus(429)
        | NetworkErrorKind::HttpStatus(500)
        | NetworkErrorKind::HttpStatus(502)
        | NetworkErrorKind::HttpStatus(503)
        | NetworkErrorKind::HttpStatus(504)
        | NetworkErrorKind::Timeout
        | NetworkErrorKind::Connect
        | NetworkErrorKind::Request => NetworkAction::Retry,
        NetworkErrorKind::HttpStatus(_) | NetworkErrorKind::Unknown => NetworkAction::Terminal,
    }
}

pub fn backoff_delay(policy: RetryPolicy, attempt: u32) -> Duration {
    if attempt == 0 {
        return Duration::ZERO;
    }
    let factor = 2u32.saturating_pow(attempt.saturating_sub(1));
    let raw_ms = policy.base_delay.as_millis().saturating_mul(u128::from(factor));
    let capped_ms = raw_ms.min(policy.max_delay.as_millis());
    Duration::from_millis(capped_ms as u64)
}

pub fn retry_cooldown_for(kind: NetworkErrorKind) -> Duration {
    match kind {
        NetworkErrorKind::HttpStatus(429) => Duration::from_secs(10 * 60),
        NetworkErrorKind::HttpStatus(503) | NetworkErrorKind::HttpStatus(504) => {
            Duration::from_secs(15 * 60)
        }
        NetworkErrorKind::HttpStatus(408)
        | NetworkErrorKind::HttpStatus(425)
        | NetworkErrorKind::HttpStatus(500)
        | NetworkErrorKind::HttpStatus(502)
        | NetworkErrorKind::Timeout
        | NetworkErrorKind::Connect
        | NetworkErrorKind::Request
        | NetworkErrorKind::Unknown => Duration::from_secs(5 * 60),
        NetworkErrorKind::HttpStatus(_) => Duration::from_secs(24 * 60 * 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_mark_404_as_terminal_removed() {
        assert_eq!(
            action_for(NetworkErrorKind::HttpStatus(404)),
            NetworkAction::TerminalRemoved
        );
    }

    #[test]
    fn should_mark_410_as_terminal_removed() {
        assert_eq!(
            action_for(NetworkErrorKind::HttpStatus(410)),
            NetworkAction::TerminalRemoved
        );
    }

    #[test]
    fn should_retry_503() {
        assert_eq!(
            action_for(NetworkErrorKind::HttpStatus(503)),
            NetworkAction::Retry
        );
    }

    #[test]
    fn should_retry_timeout() {
        assert_eq!(action_for(NetworkErrorKind::Timeout), NetworkAction::Retry);
    }

    #[test]
    fn should_backoff_exponentially_with_cap() {
        let policy = RetryPolicy {
            max_attempts: 3,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(250),
        };

        assert_eq!(backoff_delay(policy, 1), Duration::from_millis(100));
        assert_eq!(backoff_delay(policy, 2), Duration::from_millis(200));
        assert_eq!(backoff_delay(policy, 3), Duration::from_millis(250));
    }
}
