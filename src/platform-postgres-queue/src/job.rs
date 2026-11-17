//! Jobs as producers enqueue them and as workers claim them.

use crate::scope::{JobScope, QueueOrdering, ScopeName};
use std::fmt;
use time::OffsetDateTime;
use uuid::Uuid;

const MAX_KEY_BYTES: usize = 128;
/// The SQS message size limit the job bodies were designed for.
pub const MAX_PAYLOAD_BYTES: usize = 262_144;
const MAX_ERROR_CLASS_BYTES: usize = 64;

/// A job's identity. It survives retries, custody and redrive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JobId(Uuid);

impl JobId {
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, formatter)
    }
}

/// Proof of one claim. Every claim issues a new token, so a worker whose lease was taken over
/// can no longer settle the job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobLease {
    job_id: JobId,
    token: Uuid,
}

impl JobLease {
    pub(crate) const fn new(job_id: JobId, token: Uuid) -> Self {
        Self { job_id, token }
    }

    pub const fn job_id(&self) -> JobId {
        self.job_id
    }

    pub(crate) const fn token(&self) -> Uuid {
        self.token
    }
}

/// A key of printable, non-space ASCII, at most 128 bytes: the SQS group and deduplication ID
/// alphabet that existing producers already derive.
fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= MAX_KEY_BYTES && key.bytes().all(|b| b.is_ascii_graphic())
}

/// Orders the jobs of a FIFO scope.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GroupKey(String);

impl GroupKey {
    pub fn new(key: impl Into<String>) -> Result<Self, JobError> {
        let key = key.into();
        valid_key(&key)
            .then_some(Self(key))
            .ok_or(JobError::InvalidGroupKey)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Makes enqueue insert-or-nothing while an unclaimed job of the scope carries the same key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DedupKey(String);

impl DedupKey {
    pub fn new(key: impl Into<String>) -> Result<Self, JobError> {
        let key = key.into();
        valid_key(&key)
            .then_some(Self(key))
            .ok_or(JobError::InvalidDedupKey)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The bounded class of a failed attempt, such as `pool_timed_out` or `invalid_job`. It is
/// stored with the job and in custody, so it must never carry error text or payload content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorClass(String);

impl ErrorClass {
    /// Recorded when a claim finds a job whose last lease expired without a settlement.
    pub(crate) const LEASE_EXPIRED: &str = "lease_expired";

    pub fn new(class: impl Into<String>) -> Result<Self, JobError> {
        let class = class.into();
        let valid = !class.is_empty()
            && class.len() <= MAX_ERROR_CLASS_BYTES
            && class
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        valid
            .then_some(Self(class))
            .ok_or(JobError::InvalidErrorClass)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A job ready to enqueue. Construction checks it against its scope's ordering.
#[derive(Clone, PartialEq, Eq)]
pub struct NewJob {
    scope: ScopeName,
    group_key: Option<GroupKey>,
    dedup_key: Option<DedupKey>,
    payload: String,
}

impl NewJob {
    /// A FIFO scope requires a group key and a standard scope refuses one. The payload is the
    /// non-empty job body of at most [`MAX_PAYLOAD_BYTES`]; the queue never alters it.
    pub fn new(
        scope: &JobScope,
        group_key: Option<GroupKey>,
        payload: impl Into<String>,
    ) -> Result<Self, JobError> {
        match (scope.ordering(), &group_key) {
            (QueueOrdering::Fifo, None) => return Err(JobError::MissingGroupKey),
            (QueueOrdering::Standard, Some(_)) => return Err(JobError::UnexpectedGroupKey),
            _ => {}
        }
        let payload = payload.into();
        if payload.is_empty() || payload.len() > MAX_PAYLOAD_BYTES {
            return Err(JobError::InvalidPayload);
        }
        Ok(Self {
            scope: scope.name().clone(),
            group_key,
            dedup_key: None,
            payload,
        })
    }

    pub fn with_dedup_key(mut self, key: DedupKey) -> Self {
        self.dedup_key = Some(key);
        self
    }

    pub fn scope(&self) -> &ScopeName {
        &self.scope
    }

    pub fn group_key(&self) -> Option<&GroupKey> {
        self.group_key.as_ref()
    }

    pub fn dedup_key(&self) -> Option<&DedupKey> {
        self.dedup_key.as_ref()
    }

    pub fn payload(&self) -> &str {
        &self.payload
    }
}

impl fmt::Debug for NewJob {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewJob")
            .field("scope", &self.scope)
            .field("group_key", &self.group_key)
            .field("dedup_key", &self.dedup_key)
            .field("payload", &"<redacted>")
            .finish()
    }
}

/// A job leased to one worker until its lease expires or the worker settles it.
#[derive(Clone, PartialEq, Eq)]
pub struct ClaimedJob {
    pub(crate) lease: JobLease,
    pub(crate) attempt: u32,
    pub(crate) group_key: Option<GroupKey>,
    pub(crate) payload: String,
    pub(crate) enqueued_at: OffsetDateTime,
}

impl ClaimedJob {
    pub fn lease(&self) -> &JobLease {
        &self.lease
    }

    pub fn job_id(&self) -> JobId {
        self.lease.job_id()
    }

    /// This claim's attempt number, starting at 1. Greater than 1 proves an earlier delivery,
    /// not an earlier completed effect.
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    pub fn group_key(&self) -> Option<&GroupKey> {
        self.group_key.as_ref()
    }

    pub fn payload(&self) -> &str {
        &self.payload
    }

    pub fn enqueued_at(&self) -> OffsetDateTime {
        self.enqueued_at
    }
}

impl fmt::Debug for ClaimedJob {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClaimedJob")
            .field("lease", &self.lease)
            .field("attempt", &self.attempt)
            .field("group_key", &self.group_key)
            .field("payload", &"<redacted>")
            .field("enqueued_at", &self.enqueued_at)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum JobError {
    #[error("a FIFO scope's job requires a group key")]
    MissingGroupKey,
    #[error("a standard scope's job must not carry a group key")]
    UnexpectedGroupKey,
    #[error("job group key must be 1 to 128 bytes of printable ASCII without spaces")]
    InvalidGroupKey,
    #[error("job dedup key must be 1 to 128 bytes of printable ASCII without spaces")]
    InvalidDedupKey,
    #[error("job payload must be 1 to 262144 bytes")]
    InvalidPayload,
    #[error("job error class must be 1 to 64 bytes of lower-case ASCII, digits or underscores")]
    InvalidErrorClass,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn scope(ordering: QueueOrdering) -> JobScope {
        JobScope::new(
            ScopeName::new("scope").unwrap(),
            ordering,
            Duration::from_secs(30),
            5,
            Duration::from_secs(30),
        )
        .unwrap()
    }

    fn group() -> GroupKey {
        GroupKey::new("pli1_group").unwrap()
    }

    #[test]
    fn should_require_a_group_key_exactly_for_fifo_scopes() {
        let fifo = scope(QueueOrdering::Fifo);
        let standard = scope(QueueOrdering::Standard);

        assert!(NewJob::new(&fifo, Some(group()), "{}").is_ok());
        assert!(NewJob::new(&standard, None, "{}").is_ok());
        assert_eq!(
            Some(JobError::MissingGroupKey),
            NewJob::new(&fifo, None, "{}").err()
        );
        assert_eq!(
            Some(JobError::UnexpectedGroupKey),
            NewJob::new(&standard, Some(group()), "{}").err()
        );
    }

    #[test]
    fn should_bound_the_payload_without_altering_it() {
        let standard = scope(QueueOrdering::Standard);
        let largest = "x".repeat(MAX_PAYLOAD_BYTES);

        assert_eq!(
            largest,
            NewJob::new(&standard, None, largest.clone())
                .unwrap()
                .payload()
        );
        assert_eq!(
            Some(JobError::InvalidPayload),
            NewJob::new(&standard, None, "").err()
        );
        assert_eq!(
            Some(JobError::InvalidPayload),
            NewJob::new(&standard, None, format!("{largest}x")).err()
        );
    }

    #[test]
    fn should_accept_only_printable_ascii_keys_up_to_128_bytes() {
        for key in ["plid1_abc", "a", "!~", &"k".repeat(128)] {
            assert!(GroupKey::new(key).is_ok(), "{key}");
            assert!(DedupKey::new(key).is_ok(), "{key}");
        }
        for key in ["", "with space", "tab\t", "ümlaut", &"k".repeat(129)] {
            assert_eq!(Err(JobError::InvalidGroupKey), GroupKey::new(key), "{key}");
            assert_eq!(Err(JobError::InvalidDedupKey), DedupKey::new(key), "{key}");
        }
    }

    #[test]
    fn should_accept_only_bounded_lower_case_error_classes() {
        assert!(ErrorClass::new("pool_timed_out").is_ok());
        assert!(ErrorClass::new(ErrorClass::LEASE_EXPIRED).is_ok());
        for class in ["", "Upper", "with space", "dash-ed", &"e".repeat(65)] {
            assert_eq!(Err(JobError::InvalidErrorClass), ErrorClass::new(class));
        }
    }

    #[test]
    fn should_redact_payloads_from_debug_output() {
        let job = NewJob::new(&scope(QueueOrdering::Standard), None, "secret body").unwrap();

        assert!(!format!("{job:?}").contains("secret body"));
    }
}
