//! Admission of service-prepared ProductListing ingestion commands into the PostgreSQL job
//! queue's `product-listing-ingestion` FIFO scope.
//!
//! Every producer enqueues through the database function
//! `job_queue_enqueue_product_listing_ingestion`, so the crawler's role needs no table privilege
//! (`migrations/20261117140000_product_listing_ingestion_admission.sql`).
use std::{collections::HashMap, future::Future, time::Duration};

use platform_postgres_queue::{DedupKey, GroupKey, MAX_PAYLOAD_BYTES};
use product_listing_service::{
    ports::{ProductListingIngestionPublishError, ProductListingIngestionPublisher},
    use_cases::{
        ProductListingIngestionItemOutcome, ProductListingIngestionMessage,
        ProductListingIngestionNotAttemptedReason, ProductListingIngestionOutcome,
        ProductListingIngestionRejectionReason,
    },
};
use sqlx::PgPool;

use crate::codec;

const DEFAULT_BUDGET: Duration = Duration::from_secs(10);
/// SQLSTATE `invalid_parameter_value`, raised by the enqueue function for a job it refuses.
const REFUSED_JOB_SQLSTATE: &str = "22023";

/// Publisher rejection codes. Callers map them to their own reports, so they stay stable.
pub mod rejection_code {
    /// The command does not fit the queue's message limit.
    pub const INVALID_MESSAGE_SIZE: &str = "INVALID_MESSAGE_SIZE";
    /// The command cannot be encoded as a wire envelope.
    pub const ENCODING_FAILED: &str = "ENCODING_FAILED";
    /// The derived group or dedup key is not a valid queue key.
    pub const INVALID_JOB_KEY: &str = "INVALID_JOB_KEY";
    /// The queue refused the job as invalid; retrying the same job cannot succeed.
    pub const QUEUE_REFUSED: &str = "QUEUE_REFUSED";
    /// The queue certainly did not take the job, but a later attempt may.
    pub const QUEUE_RETRYABLE_FAILURE: &str = "QUEUE_RETRYABLE_FAILURE";
}

/// One encoded command as the queue stores it.
#[derive(Clone, PartialEq, Eq)]
pub struct IngestionJob {
    group_key: GroupKey,
    dedup_key: DedupKey,
    body: String,
}

impl IngestionJob {
    /// Encodes the command and derives its FIFO group and dedup keys from the wire envelope.
    fn encode(message: &ProductListingIngestionMessage) -> Result<Self, &'static str> {
        let body = codec::encode(message).map_err(|error| match error {
            codec::CodecError::TooLarge => rejection_code::INVALID_MESSAGE_SIZE,
            _ => rejection_code::ENCODING_FAILED,
        })?;
        if body.len() > MAX_PAYLOAD_BYTES {
            return Err(rejection_code::INVALID_MESSAGE_SIZE);
        }
        let envelope = codec::decode(&body).map_err(|_| rejection_code::ENCODING_FAILED)?;
        let group_key = codec::fifo_group_id(&envelope)
            .ok()
            .and_then(|key| GroupKey::new(key).ok())
            .ok_or(rejection_code::INVALID_JOB_KEY)?;
        let dedup_key = codec::fifo_deduplication_id(&envelope)
            .ok()
            .and_then(|key| DedupKey::new(key).ok())
            .ok_or(rejection_code::INVALID_JOB_KEY)?;
        Ok(Self {
            group_key,
            dedup_key,
            body,
        })
    }

    pub fn group_key(&self) -> &GroupKey {
        &self.group_key
    }

    pub fn dedup_key(&self) -> &DedupKey {
        &self.dedup_key
    }

    pub fn body(&self) -> &str {
        &self.body
    }
}

impl std::fmt::Debug for IngestionJob {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IngestionJob")
            .field("group_key", &self.group_key)
            .field("dedup_key", &self.dedup_key)
            .field("body", &"<redacted>")
            .finish()
    }
}

/// Why one enqueue did not confirm custody.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionError {
    /// The queue refused the job as invalid; it was not enqueued.
    Refused,
    /// The job was certainly not enqueued: no connection was available or the statement failed.
    NotEnqueued,
    /// The statement may have committed: the connection failed while it ran.
    Uncertain,
}

/// The single statement that hands one job to the queue.
#[async_trait::async_trait]
pub trait IngestionAdmission: Send + Sync {
    /// Confirms queue custody: the job was enqueued, or an unclaimed job with the same dedup
    /// key already holds it.
    async fn enqueue(&self, job: &IngestionJob) -> Result<(), AdmissionError>;
}

/// Enqueues through `job_queue_enqueue_product_listing_ingestion` on a pool whose role may
/// execute it: the API's runtime role or the crawler's ingestion role.
#[derive(Clone, Debug)]
pub struct PostgresIngestionAdmission {
    pool: PgPool,
}

impl PostgresIngestionAdmission {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl IngestionAdmission for PostgresIngestionAdmission {
    async fn enqueue(&self, job: &IngestionJob) -> Result<(), AdmissionError> {
        sqlx::query_scalar::<_, Option<uuid::Uuid>>(
            "SELECT job_queue_enqueue_product_listing_ingestion($1, $2, $3)",
        )
        .bind(job.group_key.as_str())
        .bind(job.dedup_key.as_str())
        .bind(&job.body)
        .fetch_one(&self.pool)
        .await
        .map(|_| ())
        .map_err(|error| classify(&error))
    }
}

/// A database error ends the statement's implicit transaction, so nothing was enqueued; only a
/// connection lost while the statement ran leaves its commit unknown. A missing grant, function
/// or login stays retryable, so it heals once fixed; the warning names its SQLSTATE.
fn classify(error: &sqlx::Error) -> AdmissionError {
    let admission = match error {
        sqlx::Error::Database(database)
            if database.code().as_deref() == Some(REFUSED_JOB_SQLSTATE) =>
        {
            AdmissionError::Refused
        }
        sqlx::Error::Database(_)
        | sqlx::Error::PoolTimedOut
        | sqlx::Error::PoolClosed
        | sqlx::Error::Configuration(_) => AdmissionError::NotEnqueued,
        _ => AdmissionError::Uncertain,
    };
    // Only the SQLSTATE and the error's kind: server messages can quote statement arguments.
    let sqlstate = error
        .as_database_error()
        .and_then(|database| database.code().map(|code| code.into_owned()));
    tracing::warn!(
        sqlstate = sqlstate.as_deref(),
        error_kind = error_kind(error),
        admission = ?admission,
        "ProductListing ingestion enqueue failed"
    );
    admission
}

fn error_kind(error: &sqlx::Error) -> &'static str {
    match error {
        sqlx::Error::Database(_) => "database",
        sqlx::Error::PoolTimedOut => "pool_timed_out",
        sqlx::Error::PoolClosed => "pool_closed",
        sqlx::Error::Configuration(_) => "configuration",
        sqlx::Error::Io(_) => "io",
        sqlx::Error::Tls(_) => "tls",
        sqlx::Error::Protocol(_) => "protocol",
        _ => "other",
    }
}

// A deadline belongs to the executing request future, never to the shared publisher.
tokio::task_local! {
    static PUBLICATION_DEADLINE: tokio::time::Instant;
}

/// Scope the absolute deadline to this future. Nested scopes may shorten, but never extend it.
pub async fn with_publication_deadline<T>(
    deadline: tokio::time::Instant,
    future: impl Future<Output = T>,
) -> T {
    let deadline = PUBLICATION_DEADLINE
        .try_with(|enclosing| deadline.min(*enclosing))
        .unwrap_or(deadline);
    PUBLICATION_DEADLINE.scope(deadline, future).await
}

/// Admits commands one at a time in input order, within its budget and any publication deadline
/// installed with [`with_publication_deadline`].
pub struct QueueProductListingIngestionPublisher<A = PostgresIngestionAdmission> {
    admission: A,
    budget: Duration,
}

impl QueueProductListingIngestionPublisher<PostgresIngestionAdmission> {
    pub fn postgres(pool: PgPool) -> Self {
        Self::new(PostgresIngestionAdmission::new(pool))
    }
}

impl<A: IngestionAdmission> QueueProductListingIngestionPublisher<A> {
    pub fn new(admission: A) -> Self {
        Self {
            admission,
            budget: DEFAULT_BUDGET,
        }
    }

    pub fn with_budget(mut self, budget: Duration) -> Self {
        self.budget = budget;
        self
    }

    fn deadline(&self) -> tokio::time::Instant {
        let budget = tokio::time::Instant::now() + self.budget;
        PUBLICATION_DEADLINE
            .try_with(|deadline| budget.min(*deadline))
            .unwrap_or(budget)
    }
}

#[async_trait::async_trait]
impl<A: IngestionAdmission> ProductListingIngestionPublisher
    for QueueProductListingIngestionPublisher<A>
{
    async fn publish(
        &self,
        commands: Vec<ProductListingIngestionMessage>,
    ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError> {
        let deadline = self.deadline();
        // Encode every command before the first enqueue. A command that cannot be encoded is
        // never sent and reserves no FIFO position, so its valid successors still go.
        let jobs: Vec<_> = commands.iter().map(IngestionJob::encode).collect();
        let outcomes = admit_in_order(&self.admission, jobs, deadline).await;
        Ok(commands
            .into_iter()
            .zip(outcomes)
            .map(|(command, outcome)| ProductListingIngestionItemOutcome {
                index: command.metadata.index,
                command_id: command.metadata.command_id,
                outcome,
            })
            .collect())
    }
}

/// Requires a deadline installed with [`with_publication_deadline`]. Use it at request
/// boundaries, where a missing deadline is a composition error rather than a license to spend
/// the publisher's full default budget.
pub struct ScopedProductListingIngestionPublisher<P> {
    publisher: P,
}

impl<P> ScopedProductListingIngestionPublisher<P> {
    pub fn new(publisher: P) -> Self {
        Self { publisher }
    }
}

#[async_trait::async_trait]
impl<P: ProductListingIngestionPublisher> ProductListingIngestionPublisher
    for ScopedProductListingIngestionPublisher<P>
{
    async fn publish(
        &self,
        commands: Vec<ProductListingIngestionMessage>,
    ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError> {
        if PUBLICATION_DEADLINE.try_with(|_| ()).is_err() {
            return Err(ProductListingIngestionPublishError::new(
                std::io::Error::other("ingestion publication deadline not installed"),
            ));
        }
        self.publisher.publish(commands).await
    }
}

/// Within a FIFO group, a successor is sent only after its predecessor's custody is confirmed:
/// a sent predecessor that failed or may have succeeded blocks the rest of its group, which
/// inherits its retryability. Independent groups continue.
async fn admit_in_order(
    admission: &impl IngestionAdmission,
    jobs: Vec<Result<IngestionJob, &'static str>>,
    deadline: tokio::time::Instant,
) -> Vec<ProductListingIngestionOutcome> {
    let mut blocked = HashMap::<GroupKey, bool>::new();
    let mut outcomes = Vec::with_capacity(jobs.len());
    for job in jobs {
        let job = match job {
            Ok(job) => job,
            Err(code) => {
                outcomes.push(rejected(code, false));
                continue;
            }
        };
        if let Some(&retryable) = blocked.get(&job.group_key) {
            outcomes.push(not_attempted(
                ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
                retryable,
            ));
            continue;
        }
        if tokio::time::Instant::now() >= deadline {
            outcomes.push(not_attempted(
                ProductListingIngestionNotAttemptedReason::DeadlineExceeded,
                true,
            ));
            continue;
        }
        let outcome = match tokio::time::timeout_at(deadline, admission.enqueue(&job)).await {
            Ok(Ok(())) => ProductListingIngestionOutcome::Accepted,
            Ok(Err(AdmissionError::Refused)) => rejected(rejection_code::QUEUE_REFUSED, false),
            Ok(Err(AdmissionError::NotEnqueued)) => {
                rejected(rejection_code::QUEUE_RETRYABLE_FAILURE, true)
            }
            // A statement cancelled by the deadline may still commit.
            Ok(Err(AdmissionError::Uncertain)) | Err(_) => {
                ProductListingIngestionOutcome::Unconfirmed
            }
        };
        if let Some(retryable) = unconfirmed_retryability(&outcome) {
            blocked.insert(job.group_key, retryable);
        }
        outcomes.push(outcome);
    }
    outcomes
}

/// The retryability of a sent job whose custody is not confirmed; `None` once it is accepted.
fn unconfirmed_retryability(outcome: &ProductListingIngestionOutcome) -> Option<bool> {
    match outcome {
        ProductListingIngestionOutcome::Accepted => None,
        ProductListingIngestionOutcome::Rejected { retryable, .. }
        | ProductListingIngestionOutcome::NotAttempted { retryable, .. } => Some(*retryable),
        ProductListingIngestionOutcome::Unconfirmed => Some(true),
    }
}

fn rejected(code: &str, retryable: bool) -> ProductListingIngestionOutcome {
    ProductListingIngestionOutcome::Rejected {
        reason: ProductListingIngestionRejectionReason::Publisher {
            code: code.to_owned(),
        },
        retryable,
    }
}

fn not_attempted(
    reason: ProductListingIngestionNotAttemptedReason,
    retryable: bool,
) -> ProductListingIngestionOutcome {
    ProductListingIngestionOutcome::NotAttempted { reason, retryable }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::{collections::VecDeque, sync::Mutex};

    const FIXTURES: &str = include_str!("../tests/fixtures/ingestion_v2.json");

    enum Reply {
        Enqueued,
        Fail(AdmissionError),
        Hang,
    }

    /// Records the bodies it is asked to enqueue and answers with scripted replies.
    struct FakeAdmission {
        replies: Mutex<VecDeque<Reply>>,
        enqueued: Mutex<Vec<String>>,
    }

    impl FakeAdmission {
        fn new(replies: impl IntoIterator<Item = Reply>) -> Self {
            Self {
                replies: Mutex::new(replies.into_iter().collect()),
                enqueued: Mutex::new(Vec::new()),
            }
        }

        fn indices(&self) -> Vec<u64> {
            self.enqueued
                .lock()
                .unwrap()
                .iter()
                .map(|body| {
                    serde_json::from_str::<Value>(body).unwrap()["index"]
                        .as_u64()
                        .unwrap()
                })
                .collect()
        }
    }

    #[async_trait::async_trait]
    impl IngestionAdmission for FakeAdmission {
        async fn enqueue(&self, job: &IngestionJob) -> Result<(), AdmissionError> {
            self.enqueued.lock().unwrap().push(job.body().to_owned());
            let reply = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected enqueue");
            match reply {
                Reply::Enqueued => Ok(()),
                Reply::Fail(error) => Err(error),
                Reply::Hang => std::future::pending().await,
            }
        }
    }

    /// Fixtures 0 to 3 change one listing (one FIFO group); fixture 4 captures raw evidence in
    /// its own group.
    fn commands(indices: &[usize]) -> Vec<ProductListingIngestionMessage> {
        let fixtures: Vec<Value> = serde_json::from_str(FIXTURES).unwrap();
        indices
            .iter()
            .map(|&index| {
                codec::decode(&fixtures[index].to_string())
                    .unwrap()
                    .into_service_envelope()
                    .unwrap()
                    .message
            })
            .collect()
    }

    async fn publish(
        admission: FakeAdmission,
        commands: Vec<ProductListingIngestionMessage>,
    ) -> (Vec<ProductListingIngestionOutcome>, Vec<u64>) {
        let publisher = QueueProductListingIngestionPublisher::new(admission);
        let outcomes = publisher
            .publish(commands)
            .await
            .unwrap()
            .into_iter()
            .map(|item| item.outcome)
            .collect();
        (outcomes, publisher.admission.indices())
    }

    fn blocked(retryable: bool) -> ProductListingIngestionOutcome {
        not_attempted(
            ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
            retryable,
        )
    }

    fn deadline_exceeded() -> ProductListingIngestionOutcome {
        not_attempted(
            ProductListingIngestionNotAttemptedReason::DeadlineExceeded,
            true,
        )
    }

    #[derive(Debug)]
    struct ServerError(&'static str);

    impl std::fmt::Display for ServerError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("server error")
        }
    }

    impl std::error::Error for ServerError {}

    impl sqlx::error::DatabaseError for ServerError {
        fn message(&self) -> &str {
            "server error"
        }

        fn code(&self) -> Option<std::borrow::Cow<'_, str>> {
            Some(self.0.into())
        }

        fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
            self
        }

        fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
            self
        }

        fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
            self
        }

        fn kind(&self) -> sqlx::error::ErrorKind {
            sqlx::error::ErrorKind::Other
        }
    }

    #[test]
    fn should_classify_only_a_lost_connection_as_an_uncertain_enqueue() {
        let database = |code| sqlx::Error::Database(Box::new(ServerError(code)));

        assert_eq!(AdmissionError::Refused, classify(&database("22023")));
        for not_enqueued in [
            database("42501"),
            database("40P01"),
            sqlx::Error::PoolTimedOut,
            sqlx::Error::PoolClosed,
        ] {
            assert_eq!(AdmissionError::NotEnqueued, classify(&not_enqueued));
        }
        for uncertain in [
            sqlx::Error::Io(std::io::Error::other("connection reset")),
            sqlx::Error::Protocol("unexpected message".into()),
        ] {
            assert_eq!(AdmissionError::Uncertain, classify(&uncertain));
        }
    }

    #[test]
    fn should_key_jobs_by_listing_group_and_command_identity() {
        let jobs: Vec<_> = commands(&[0, 1, 4])
            .iter()
            .map(|command| IngestionJob::encode(command).unwrap())
            .collect();

        assert_eq!(jobs[0].group_key(), jobs[1].group_key());
        assert_ne!(jobs[0].group_key(), jobs[2].group_key());
        assert_ne!(jobs[0].dedup_key(), jobs[1].dedup_key());
        assert!(jobs[0].group_key().as_str().starts_with("pli1_"));
        assert!(jobs[0].dedup_key().as_str().starts_with("plid1_"));
        assert!(!format!("{:?}", jobs[0]).contains(jobs[0].body()));
    }

    #[tokio::test]
    async fn should_admit_in_input_order_and_hold_a_group_behind_an_unconfirmed_predecessor() {
        let admission = FakeAdmission::new([
            Reply::Enqueued,
            Reply::Fail(AdmissionError::Uncertain),
            Reply::Enqueued,
        ]);

        let (outcomes, sent) = publish(admission, commands(&[0, 1, 2, 4])).await;

        assert_eq!(
            vec![
                ProductListingIngestionOutcome::Accepted,
                ProductListingIngestionOutcome::Unconfirmed,
                blocked(true),
                ProductListingIngestionOutcome::Accepted,
            ],
            outcomes
        );
        assert_eq!(vec![0, 1, 4], sent);
    }

    #[tokio::test]
    async fn should_pass_a_failed_predecessor_retryability_to_its_blocked_group() {
        let admission = FakeAdmission::new([
            Reply::Fail(AdmissionError::Refused),
            Reply::Fail(AdmissionError::NotEnqueued),
        ]);

        let (outcomes, sent) = publish(admission, commands(&[0, 1, 4])).await;

        assert_eq!(
            vec![
                rejected(rejection_code::QUEUE_REFUSED, false),
                blocked(false),
                rejected(rejection_code::QUEUE_RETRYABLE_FAILURE, true),
            ],
            outcomes
        );
        assert_eq!(vec![0, 4], sent);
    }

    #[tokio::test]
    async fn should_not_reserve_a_fifo_position_for_a_command_that_cannot_be_encoded() {
        let mut commands = commands(&[0, 1]);
        // Metadata that contradicts the intent cannot be encoded.
        commands[0].metadata.operation = commands[1].metadata.operation;
        let admission = FakeAdmission::new([Reply::Enqueued]);

        let (outcomes, sent) = publish(admission, commands).await;

        assert_eq!(
            vec![
                rejected(rejection_code::ENCODING_FAILED, false),
                ProductListingIngestionOutcome::Accepted,
            ],
            outcomes
        );
        assert_eq!(vec![1], sent);
    }

    #[tokio::test(start_paused = true)]
    async fn should_report_an_enqueue_cut_off_by_the_deadline_as_unconfirmed() {
        let admission = FakeAdmission::new([Reply::Hang]);
        let publisher = QueueProductListingIngestionPublisher::new(admission)
            .with_budget(Duration::from_secs(1));

        let outcomes: Vec<_> = publisher
            .publish(commands(&[0, 4]))
            .await
            .unwrap()
            .into_iter()
            .map(|item| item.outcome)
            .collect();

        assert_eq!(
            vec![
                ProductListingIngestionOutcome::Unconfirmed,
                deadline_exceeded()
            ],
            outcomes
        );
    }

    #[tokio::test(start_paused = true)]
    async fn should_let_a_request_deadline_shorten_but_never_extend_the_budget() {
        let publisher = QueueProductListingIngestionPublisher::new(FakeAdmission::new([]));
        let expired = tokio::time::Instant::now() - Duration::from_millis(1);

        let outcomes = with_publication_deadline(expired, async {
            with_publication_deadline(
                tokio::time::Instant::now() + Duration::from_secs(60),
                publisher.publish(commands(&[0])),
            )
            .await
        })
        .await
        .unwrap();

        assert_eq!(deadline_exceeded(), outcomes[0].outcome);
        assert!(publisher.admission.indices().is_empty());
    }

    #[tokio::test]
    async fn should_refuse_a_scoped_publication_without_a_request_deadline() {
        let scoped = ScopedProductListingIngestionPublisher::new(
            QueueProductListingIngestionPublisher::new(FakeAdmission::new([])),
        );

        assert!(scoped.publish(commands(&[0])).await.is_err());
        let outcomes = with_publication_deadline(
            tokio::time::Instant::now() + Duration::from_secs(1),
            scoped.publish(Vec::new()),
        )
        .await
        .unwrap();
        assert!(outcomes.is_empty());
    }
}
