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
            base_delay: Duration::from_millis(1000),
            max_delay: Duration::from_secs(2),
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
        NetworkErrorKind::HttpStatus(403)
        | NetworkErrorKind::HttpStatus(408)
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

pub fn is_retryable_network_failure(kind: NetworkErrorKind) -> bool {
    action_for(kind) == NetworkAction::Retry
}

pub fn should_adapt_domain_delay(kind: NetworkErrorKind) -> bool {
    matches!(
        kind,
        NetworkErrorKind::HttpStatus(408)
            | NetworkErrorKind::HttpStatus(429)
            | NetworkErrorKind::HttpStatus(503)
            | NetworkErrorKind::HttpStatus(504)
            | NetworkErrorKind::Timeout
            | NetworkErrorKind::Connect
    )
}

pub fn inline_retry_backoff_for(policy: RetryPolicy, attempt: u32) -> Duration {
    if attempt == 0 {
        return Duration::ZERO;
    }
    let factor = 2u32.saturating_pow(attempt.saturating_sub(1));
    let raw_ms = policy
        .base_delay
        .as_millis()
        .saturating_mul(u128::from(factor));
    let capped_ms = raw_ms.min(policy.max_delay.as_millis());
    Duration::from_millis(capped_ms as u64)
}

/// Durable cooldown persisted after all inline fetch attempts fail.
///
/// Do not use this inside a domain worker between fetch attempts; use
/// [`inline_retry_backoff_for`] there so one slow shop cannot block the worker
/// for minutes.
pub fn durable_retry_cooldown_for(kind: NetworkErrorKind) -> Duration {
    match kind {
        NetworkErrorKind::HttpStatus(429) => Duration::from_secs(10),
        NetworkErrorKind::HttpStatus(503) | NetworkErrorKind::HttpStatus(504) => {
            Duration::from_secs(15 * 60)
        }
        NetworkErrorKind::HttpStatus(403) => Duration::from_secs(2 * 60),
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
    fn should_retry_403() {
        assert_eq!(
            action_for(NetworkErrorKind::HttpStatus(403)),
            NetworkAction::Retry
        );
    }

    #[test]
    fn should_calculate_durable_cooldown_403_for_two_minutes() {
        assert_eq!(
            durable_retry_cooldown_for(NetworkErrorKind::HttpStatus(403)),
            Duration::from_secs(2 * 60)
        );
    }

    #[test]
    fn should_retry_timeout() {
        assert_eq!(action_for(NetworkErrorKind::Timeout), NetworkAction::Retry);
    }

    #[test]
    fn should_identify_retryable_network_failures() {
        assert!(is_retryable_network_failure(NetworkErrorKind::Timeout));
        assert!(is_retryable_network_failure(NetworkErrorKind::Connect));
        assert!(is_retryable_network_failure(NetworkErrorKind::Request));
        assert!(is_retryable_network_failure(NetworkErrorKind::HttpStatus(
            429
        )));
        assert!(is_retryable_network_failure(NetworkErrorKind::HttpStatus(
            500
        )));
        assert!(is_retryable_network_failure(NetworkErrorKind::HttpStatus(
            503
        )));
    }

    #[test]
    fn should_adapt_domain_delay_for_domain_health_signals() {
        assert!(should_adapt_domain_delay(NetworkErrorKind::Timeout));
        assert!(should_adapt_domain_delay(NetworkErrorKind::Connect));
        assert!(should_adapt_domain_delay(NetworkErrorKind::HttpStatus(408)));
        assert!(should_adapt_domain_delay(NetworkErrorKind::HttpStatus(429)));
        assert!(should_adapt_domain_delay(NetworkErrorKind::HttpStatus(503)));
        assert!(should_adapt_domain_delay(NetworkErrorKind::HttpStatus(504)));
    }

    #[test]
    fn should_not_adapt_domain_delay_for_url_scoped_retryable_failures() {
        assert!(!should_adapt_domain_delay(NetworkErrorKind::Request));
        assert!(!should_adapt_domain_delay(NetworkErrorKind::HttpStatus(
            425
        )));
        assert!(!should_adapt_domain_delay(NetworkErrorKind::HttpStatus(
            500
        )));
        assert!(!should_adapt_domain_delay(NetworkErrorKind::HttpStatus(
            502
        )));
    }

    #[test]
    fn should_not_identify_terminal_network_failures_as_retryable() {
        assert!(!is_retryable_network_failure(NetworkErrorKind::HttpStatus(
            404
        )));
        assert!(!is_retryable_network_failure(NetworkErrorKind::HttpStatus(
            410
        )));
        assert!(!is_retryable_network_failure(NetworkErrorKind::HttpStatus(
            418
        )));
        assert!(!is_retryable_network_failure(NetworkErrorKind::Unknown));
    }

    #[test]
    fn should_calculate_short_inline_retry_backoff_exponentially_with_cap() {
        let policy = RetryPolicy {
            max_attempts: 3,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(250),
        };

        assert_eq!(
            inline_retry_backoff_for(policy, 1),
            Duration::from_millis(100)
        );
        assert_eq!(
            inline_retry_backoff_for(policy, 2),
            Duration::from_millis(200)
        );
        assert_eq!(
            inline_retry_backoff_for(policy, 3),
            Duration::from_millis(250)
        );
    }

    #[test]
    fn should_cap_default_inline_retry_backoff_at_two_seconds() {
        let policy = RetryPolicy::default();

        assert_eq!(inline_retry_backoff_for(policy, 3), Duration::from_secs(2));
    }
}
