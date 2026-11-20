//! A scope is one logical queue: its name, ordering and delivery policy.

use std::{fmt, time::Duration};

const MAX_SCOPE_NAME_BYTES: usize = 48;
const NOTIFY_CHANNEL_PREFIX: &str = "job_queue:";
/// Bounds a lease or retry delay so its interval arithmetic stays far from overflow.
const MAX_DELAY: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_ATTEMPTS: u32 = 100;

/// A scope's persisted and notified name: lower-case ASCII words joined by single hyphens, at
/// most 48 bytes, so its notification channel stays within PostgreSQL's identifier limit.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScopeName(String);

impl ScopeName {
    pub fn new(name: impl Into<String>) -> Result<Self, ScopeError> {
        let name = name.into();
        let valid = !name.is_empty()
            && name.len() <= MAX_SCOPE_NAME_BYTES
            && name.split('-').all(|word| {
                !word.is_empty()
                    && word
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            });
        valid.then_some(Self(name)).ok_or(ScopeError::InvalidName)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The `LISTEN/NOTIFY` channel that announces newly enqueued jobs of this scope.
    pub fn notify_channel(&self) -> String {
        format!("{NOTIFY_CHANNEL_PREFIX}{}", self.0)
    }

    pub(crate) fn from_notify_channel(channel: &str) -> Option<Self> {
        Self::new(channel.strip_prefix(NOTIFY_CHANNEL_PREFIX)?).ok()
    }
}

impl fmt::Debug for ScopeName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, formatter)
    }
}

impl fmt::Display for ScopeName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// How a scope releases its jobs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueOrdering {
    /// Any claimable job may run, concurrently with any other.
    Standard,
    /// Every job carries a group key. Within a group only the oldest job is claimable, and only
    /// while no job of that group is leased.
    Fifo,
}

/// One scope and its delivery policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobScope {
    name: ScopeName,
    ordering: QueueOrdering,
    lease: Duration,
    max_attempts: u32,
    retry_delay: Duration,
}

impl JobScope {
    /// - `lease`: how long a claim holds a job before another worker may claim it again.
    /// - `max_attempts`: claims a job gets before it moves to failure custody.
    /// - `retry_delay`: the backoff before a job released for retry becomes claimable again.
    pub fn new(
        name: ScopeName,
        ordering: QueueOrdering,
        lease: Duration,
        max_attempts: u32,
        retry_delay: Duration,
    ) -> Result<Self, ScopeError> {
        if lease < Duration::from_millis(1) || lease > MAX_DELAY {
            return Err(ScopeError::InvalidLease);
        }
        if retry_delay > MAX_DELAY {
            return Err(ScopeError::InvalidRetryDelay);
        }
        if !(1..=MAX_ATTEMPTS).contains(&max_attempts) {
            return Err(ScopeError::InvalidMaxAttempts);
        }
        Ok(Self {
            name,
            ordering,
            lease,
            max_attempts,
            retry_delay,
        })
    }

    pub fn name(&self) -> &ScopeName {
        &self.name
    }

    pub fn ordering(&self) -> QueueOrdering {
        self.ordering
    }

    pub fn lease(&self) -> Duration {
        self.lease
    }

    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    pub fn retry_delay(&self) -> Duration {
        self.retry_delay
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ScopeError {
    #[error("job scope name must be at most 48 bytes of hyphen-separated lower-case ASCII words")]
    InvalidName,
    #[error("job scope lease must be between one millisecond and one day")]
    InvalidLease,
    #[error("job scope retry delay must be at most one day")]
    InvalidRetryDelay,
    #[error("job scope must allow between 1 and 100 attempts")]
    InvalidMaxAttempts,
}

/// Microseconds for PostgreSQL interval arithmetic; scope validation bounds every duration.
pub(crate) fn micros(duration: Duration) -> i64 {
    i64::try_from(duration.as_micros()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_accept_hyphenated_lower_case_names_up_to_48_bytes() {
        for name in [
            "product-listing-opensearch",
            "a",
            "geo-resolution2",
            &"a".repeat(48),
        ] {
            assert_eq!(name, ScopeName::new(name).unwrap().as_str());
        }
    }

    #[test]
    fn should_reject_names_that_could_not_be_persisted_or_notified() {
        for name in [
            "",
            "-leading",
            "trailing-",
            "double--hyphen",
            "Upper",
            "under_score",
            "dot.ted",
            "spa ce",
            &"a".repeat(49),
        ] {
            assert_eq!(Err(ScopeError::InvalidName), ScopeName::new(name), "{name}");
        }
    }

    #[test]
    fn should_round_trip_the_scope_through_its_notify_channel() {
        let scope = ScopeName::new("listing-shipping").unwrap();

        assert_eq!("job_queue:listing-shipping", scope.notify_channel());
        assert_eq!(
            Some(scope.clone()),
            ScopeName::from_notify_channel(&scope.notify_channel())
        );
        assert_eq!(None, ScopeName::from_notify_channel("listing-shipping"));
        assert_eq!(None, ScopeName::from_notify_channel("job_queue:Bad"));
    }

    #[test]
    fn should_reject_policies_outside_the_supported_bounds() {
        let name = ScopeName::new("scope").unwrap();
        let build = |lease, attempts, retry| {
            JobScope::new(
                name.clone(),
                QueueOrdering::Standard,
                lease,
                attempts,
                retry,
            )
        };
        let second = Duration::from_secs(1);

        assert!(build(second, 5, second).is_ok());
        assert!(build(second, 1, Duration::ZERO).is_ok());
        assert_eq!(
            Err(ScopeError::InvalidLease),
            build(Duration::ZERO, 5, second)
        );
        assert_eq!(
            Err(ScopeError::InvalidLease),
            build(MAX_DELAY + second, 5, second)
        );
        assert_eq!(
            Err(ScopeError::InvalidRetryDelay),
            build(second, 5, MAX_DELAY + second)
        );
        assert_eq!(
            Err(ScopeError::InvalidMaxAttempts),
            build(second, 0, second)
        );
        assert_eq!(
            Err(ScopeError::InvalidMaxAttempts),
            build(second, 101, second)
        );
    }
}
