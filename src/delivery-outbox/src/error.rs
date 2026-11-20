use platform_postgres_queue::JobQueueError;

#[derive(Debug, thiserror::Error)]
pub enum OutboxError {
    #[error("delivery outbox database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("delivery outbox could not enqueue a job")]
    Enqueue(#[from] JobQueueError),
}
