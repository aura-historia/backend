use platform_postgres_queue::{ClaimedJob, GroupKey, JobId};
use std::fmt;

/// What a handler sees of a claimed job. The lease stays with the consumer, so only the
/// consumer settles a job.
#[derive(Clone, PartialEq, Eq)]
pub struct Job {
    id: JobId,
    attempt: u32,
    group_key: Option<GroupKey>,
    payload: String,
}

impl Job {
    /// A job as a handler receives it; also how handler tests build one.
    pub fn new(id: JobId, attempt: u32, group_key: Option<GroupKey>, payload: String) -> Self {
        Self {
            id,
            attempt,
            group_key,
            payload,
        }
    }

    pub fn id(&self) -> JobId {
        self.id
    }

    /// The claim's attempt number, starting at 1. Greater than 1 proves an earlier delivery,
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
}

impl From<ClaimedJob> for Job {
    fn from(job: ClaimedJob) -> Self {
        Self {
            id: job.job_id(),
            attempt: job.attempt(),
            group_key: job.group_key().cloned(),
            payload: job.payload().to_owned(),
        }
    }
}

impl fmt::Debug for Job {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Job")
            .field("id", &self.id)
            .field("attempt", &self.attempt)
            .field("group_key", &self.group_key)
            .field("payload", &"<redacted>")
            .finish()
    }
}
