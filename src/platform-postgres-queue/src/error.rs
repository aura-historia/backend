/// Why a queue operation failed. Neither variant carries payloads, keys or server messages.
#[derive(Debug, thiserror::Error)]
pub enum JobQueueError {
    /// The statement or transaction failed, or its outcome is unknown: a lost commit may still
    /// have applied. Callers retry the operation; the queue's lease rules keep that safe.
    #[error("job queue database operation failed")]
    Database(#[source] sqlx::Error),
    /// A stored row violates the queue's own invariants; it is left untouched for repair.
    #[error("job queue holds invalid persisted state: {0}")]
    InvalidPersistedState(&'static str),
}

impl From<sqlx::Error> for JobQueueError {
    fn from(source: sqlx::Error) -> Self {
        Self::Database(source)
    }
}
