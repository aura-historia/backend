//! One round of batched product embedding: collect ended provider batches and commit their
//! vectors under the embedding source-marker guard, then submit pending work as a new batch.
//!
//! Work is durable in the embedding queue, so a round that fails part-way leaves every
//! embedding pending, submitted or settled, and the next round continues from there.

use super::product_embedding_work::{
    EmbeddingDocument, ProductEmbeddingSettlements, ProductEmbeddingSettler,
    ProductEmbeddingWorkError, Retry, after_error, commit, failed,
};
use crate::ports::{
    BatchedProductEmbedding, ProductEmbeddingAssignment, ProductEmbeddingBatchKey,
    ProductEmbeddingClaim, ProductListingEmbeddingQueue, ProductListingEmbeddingQueueFactory,
    ProductListingEmbeddingQueueReader, ProductListingEmbeddingWriterFactory,
    SubmittedProductEmbeddingBatch,
};
use application::error::box_error;
use application::operation_context::OperationContext;
use application::transaction::UnitOfWork;
use embedding::{
    EmbeddingBatchId, EmbeddingBatchPoll, EmbeddingError, EmbeddingVector, EndedEmbeddingBatch,
    ProductEmbeddingBatches, ProductEmbeddingRequest, UnansweredEmbeddings,
};
use std::collections::BTreeMap;
use std::time::Duration;

/// Embeddings one batch submits at most. Every request's image is fetched and staged while the
/// batch is submitted, which bounds a round's duration.
pub const MAX_EMBEDDINGS_PER_BATCH: u32 = 10_000;
/// A batch whose submission was not recorded within this time never reached the provider, or
/// reached it without its ID being kept; either way its work returns to the queue. It exceeds
/// the time staging a full batch's images takes.
const ABANDONED_SUBMISSION_AGE: Duration = Duration::from_secs(2 * 60 * 60);

type Error = ProductEmbeddingWorkError;

/// What one round did, for its log record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProductEmbeddingRoundReport {
    pub abandoned_batches: u64,
    pub batches_in_progress: usize,
    pub batches_unavailable: usize,
    pub batches_collected: usize,
    /// Batches whose collection failed this round; the next round retries them.
    pub batches_failed: usize,
    pub settlements: ProductEmbeddingSettlements,
    pub superseded_discarded: u64,
    pub submitted_embeddings: usize,
}

#[async_trait::async_trait]
pub trait EmbedProductListingBatchesUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
    ) -> Result<ProductEmbeddingRoundReport, ProductEmbeddingWorkError>;
}

pub struct EmbedProductListingBatchesHandler<R, B, U, Q, W> {
    reader: R,
    batches: B,
    settler: ProductEmbeddingSettler<U, Q, W>,
}

impl<R, B, U, Q, W> EmbedProductListingBatchesHandler<R, B, U, Q, W> {
    pub fn new(reader: R, batches: B, unit_of_work: U, queue: Q, embeddings: W) -> Self {
        Self {
            reader,
            batches,
            settler: ProductEmbeddingSettler {
                unit_of_work,
                queue,
                embeddings,
            },
        }
    }
}

#[async_trait::async_trait]
impl<R, B, U, Q, W> EmbedProductListingBatchesUseCase
    for EmbedProductListingBatchesHandler<R, B, U, Q, W>
where
    R: ProductListingEmbeddingQueueReader,
    B: ProductEmbeddingBatches,
    U: UnitOfWork,
    Q: ProductListingEmbeddingQueueFactory<U::Tx>,
    W: ProductListingEmbeddingWriterFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "embed_product_batches",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
    ) -> Result<ProductEmbeddingRoundReport, Error> {
        context.require().system().authorize::<Error>()?;
        let mut report = ProductEmbeddingRoundReport {
            abandoned_batches: self.close_abandoned_submissions().await?,
            ..Default::default()
        };
        // One batch that cannot be collected must not hold back the others or new work.
        for batch in self.reader.submitted_batches().await? {
            if let Err(error) = self.collect(&batch, &mut report).await {
                report.batches_failed += 1;
                tracing::warn!(
                    event = "product.embedding.batch_failed",
                    provider_batch_id = %batch.provider_batch_id,
                    error_kind = error.kind(),
                    "Embedding batch collection failed; the next round retries it"
                );
            }
        }
        self.submit_pending(&mut report).await?;
        Ok(report)
    }
}

impl<R, B, U, Q, W> EmbedProductListingBatchesHandler<R, B, U, Q, W>
where
    R: ProductListingEmbeddingQueueReader,
    B: ProductEmbeddingBatches,
    U: UnitOfWork,
    Q: ProductListingEmbeddingQueueFactory<U::Tx>,
    W: ProductListingEmbeddingWriterFactory<U::Tx>,
{
    /// Settles every current item of an ended batch, then closes it. A batch whose results
    /// cannot be read now is left for the next round; one whose results the provider rejected
    /// for good fails every item once.
    async fn collect(
        &self,
        batch: &SubmittedProductEmbeddingBatch,
        report: &mut ProductEmbeddingRoundReport,
    ) -> Result<(), Error> {
        let ended = match self.batches.poll(&batch.provider_batch_id).await {
            Ok(EmbeddingBatchPoll::InProgress) => {
                report.batches_in_progress += 1;
                return Ok(());
            }
            Ok(EmbeddingBatchPoll::Ended(ended)) => ended,
            Err(error) if error.is_transient() => {
                report.batches_unavailable += 1;
                return Ok(());
            }
            Err(error) => {
                tracing::warn!(
                    event = "product.embedding.batch_unreadable",
                    provider_batch_id = %batch.provider_batch_id,
                    error_category = error.diagnostic_category(),
                    "Embedding batch results were rejected; its embeddings are retried"
                );
                EndedEmbeddingBatch {
                    results: Vec::new(),
                    unanswered: UnansweredEmbeddings::Failed,
                }
            }
        };
        let items = self.reader.batch_items(batch.key).await?;
        let size = items.len();
        let answered = ended.results.len();
        let claim = ProductEmbeddingClaim::Batch(batch.key);
        let mut settlements = ProductEmbeddingSettlements::default();
        for (item, result) in settlements_of(items, ended) {
            self.settler
                .settle(&item.embedding, claim, result, &mut settlements)
                .await?;
        }
        self.close_batch(batch.key).await?;
        log_collected(
            &batch.provider_batch_id,
            batch.age,
            size,
            answered,
            &settlements,
        );
        report.batches_collected += 1;
        report.settlements += settlements;
        Ok(())
    }

    async fn submit_pending(&self, report: &mut ProductEmbeddingRoundReport) -> Result<(), Error> {
        let key = ProductEmbeddingBatchKey::new();
        let mut tx = self.settler.begin().await?;
        let requests = {
            let mut queue = self.settler.queue.in_transaction(&mut tx);
            report.superseded_discarded = queue.discard_superseded().await?;
            let mut requests = Vec::new();
            let mut assignments = Vec::new();
            for pending in queue.lock_pending(MAX_EMBEDDINGS_PER_BATCH).await? {
                let Ok(document) = EmbeddingDocument::of(&pending) else {
                    // No provider embeds this document; only a content change can fix it.
                    let attempts = pending.attempts.saturating_add(1);
                    queue
                        .park(pending.work, ProductEmbeddingClaim::Pending, attempts)
                        .await?;
                    report.settlements.parked += 1;
                    continue;
                };
                let position = u32::try_from(requests.len() + 1).unwrap_or(u32::MAX);
                assignments.push(ProductEmbeddingAssignment {
                    work: pending.work,
                    position,
                });
                requests.push(ProductEmbeddingRequest {
                    key: position,
                    title: document.title,
                    additional_text: document.additional_text,
                    image_url: document.image_url,
                });
            }
            if !assignments.is_empty() {
                queue.create_batch(key, &assignments).await?;
            }
            requests
        };
        commit(tx).await?;
        if requests.is_empty() {
            return Ok(());
        }
        match self
            .batches
            .submit(&key.as_uuid().to_string(), &requests)
            .await
        {
            Ok(provider_batch_id) => {
                self.record_submission(key, &provider_batch_id).await?;
                tracing::info!(
                    event = "product.embedding.batch.submitted",
                    provider_batch_id = %provider_batch_id,
                    size = requests.len(),
                );
                report.submitted_embeddings = requests.len();
                Ok(())
            }
            Err(error) => {
                self.close_batch(key).await?;
                Err(Error::SubmissionFailed {
                    source: box_error(error),
                })
            }
        }
    }

    async fn close_abandoned_submissions(&self) -> Result<u64, Error> {
        let mut tx = self.settler.begin().await?;
        let closed = self
            .settler
            .queue
            .in_transaction(&mut tx)
            .close_abandoned_submissions(ABANDONED_SUBMISSION_AGE)
            .await?;
        commit(tx).await?;
        Ok(closed)
    }

    async fn record_submission(
        &self,
        key: ProductEmbeddingBatchKey,
        provider_batch_id: &EmbeddingBatchId,
    ) -> Result<(), Error> {
        let mut tx = self.settler.begin().await?;
        self.settler
            .queue
            .in_transaction(&mut tx)
            .record_submission(key, provider_batch_id)
            .await?;
        commit(tx).await
    }

    async fn close_batch(&self, key: ProductEmbeddingBatchKey) -> Result<(), Error> {
        let mut tx = self.settler.begin().await?;
        self.settler
            .queue
            .in_transaction(&mut tx)
            .close_batch(key)
            .await?;
        commit(tx).await
    }
}

/// Decides each current item's settlement from its request's result. A request the provider
/// did not answer settles as the batch's end state says.
fn settlements_of(
    items: Vec<BatchedProductEmbedding>,
    ended: EndedEmbeddingBatch,
) -> Vec<(BatchedProductEmbedding, Result<EmbeddingVector, Retry>)> {
    let mut results = ended
        .results
        .into_iter()
        .collect::<BTreeMap<u32, Result<EmbeddingVector, EmbeddingError>>>();
    items
        .into_iter()
        .map(|item| {
            let pending = &item.embedding;
            let settlement = match results.remove(&item.position) {
                Some(Ok(vector)) => Ok(vector),
                Some(Err(error)) => Err(after_error(pending, &error)),
                None => Err(match ended.unanswered {
                    UnansweredEmbeddings::NotProcessed => Retry::Requeue(pending.attempts),
                    UnansweredEmbeddings::Failed => failed(pending),
                }),
            };
            (item, settlement)
        })
        .collect()
}

/// The per-job record: its size, how its results settled and its turnaround, measured until
/// the round that collected it.
fn log_collected(
    provider_batch_id: &EmbeddingBatchId,
    age: Duration,
    size: usize,
    answered: usize,
    settlements: &ProductEmbeddingSettlements,
) {
    tracing::info!(
        event = "product.embedding.batch.collected",
        provider_batch_id = %provider_batch_id,
        size,
        answered,
        applied = settlements.applied,
        duplicate = settlements.duplicate,
        stale = settlements.stale,
        requeued = settlements.requeued,
        parked = settlements.parked,
        write_failures = settlements.write_failures,
        turnaround_seconds = age.as_secs(),
    );
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{PROVIDER_BATCH, World, system, vector};
    use super::*;
    use crate::ports::ProductListingEmbeddingWriteOutcome;
    use crate::use_cases::MAX_EMBEDDING_ATTEMPTS;
    use embedding::EmbeddingText;

    type Handler = EmbedProductListingBatchesHandler<World, World, World, World, World>;

    fn handler(world: &World) -> Handler {
        EmbedProductListingBatchesHandler::new(
            world.clone(),
            world.clone(),
            world.clone(),
            world.clone(),
            world.clone(),
        )
    }

    fn ended(
        results: Vec<(u32, Result<EmbeddingVector, EmbeddingError>)>,
        unanswered: UnansweredEmbeddings,
    ) -> Result<EmbeddingBatchPoll, EmbeddingError> {
        Ok(EmbeddingBatchPoll::Ended(EndedEmbeddingBatch {
            results,
            unanswered,
        }))
    }

    #[tokio::test]
    async fn should_submit_pending_work_with_its_document_and_record_the_batch() {
        let world = World::default();
        let first = world.add_pending("Ancient vase");
        let second = world.add_pending("Brass lamp");

        let report = handler(&world).execute(&system()).await.unwrap();

        assert_eq!(2, report.submitted_embeddings);
        let state = world.state();
        let [(label, requests)] = state.submitted.as_slice() else {
            panic!("one batch is submitted");
        };
        let (key, provider_batch_id) = &state.batches[0];
        assert_eq!(&key.as_uuid().to_string(), label);
        assert_eq!(
            Some(PROVIDER_BATCH),
            provider_batch_id.as_ref().map(|id| id.as_str())
        );
        assert_eq!(
            vec![
                (1, "Ancient vase", Some("Painted clay")),
                (2, "Brass lamp", Some("Painted clay"))
            ],
            requests
                .iter()
                .map(|request| (
                    request.key,
                    request.title.as_str(),
                    request.additional_text.as_ref().map(EmbeddingText::as_str)
                ))
                .collect::<Vec<_>>()
        );
        let key = *key;
        drop(state);
        assert_eq!(Some((key, 1)), world.row(first).unwrap().batch);
        assert_eq!(Some((key, 2)), world.row(second).unwrap().batch);
    }

    #[tokio::test]
    async fn should_commit_answered_vectors_and_complete_their_work() {
        let world = World::default();
        let work = world.add_pending("Ancient vase");
        world.submitted(&[work]);
        world
            .state()
            .polls
            .push_back(ended(vec![(1, Ok(vector()))], UnansweredEmbeddings::Failed));

        let report = handler(&world).execute(&system()).await.unwrap();

        assert_eq!(1, report.batches_collected);
        assert_eq!(1, report.settlements.applied);
        assert!(world.row(work).is_none());
        assert!(world.state().batches.is_empty());
        let state = world.state();
        assert_eq!(work.source_event_id, state.writes[0].source_event_id);
    }

    #[tokio::test]
    async fn should_drop_a_result_for_a_superseded_revision_as_stale() {
        let world = World::default();
        let work = world.add_pending("Ancient vase");
        world.submitted(&[work]);
        {
            let mut state = world.state();
            state.write_outcome = Some(ProductListingEmbeddingWriteOutcome::Stale);
            state
                .polls
                .push_back(ended(vec![(1, Ok(vector()))], UnansweredEmbeddings::Failed));
        }

        let report = handler(&world).execute(&system()).await.unwrap();

        assert_eq!(
            (0, 1),
            (report.settlements.applied, report.settlements.stale)
        );
        assert!(world.row(work).is_none());
    }

    #[tokio::test]
    async fn should_requeue_unprocessed_requests_without_spending_an_attempt() {
        let world = World::default();
        let answered = world.add_pending("Ancient vase");
        let unprocessed = world.add_pending("Brass lamp");
        world.submitted(&[answered, unprocessed]);
        world.state().polls.push_back(ended(
            vec![(1, Ok(vector()))],
            UnansweredEmbeddings::NotProcessed,
        ));
        world.state().submission_fails = true;

        let result = handler(&world).execute(&system()).await;

        assert!(matches!(result, Err(Error::SubmissionFailed { .. })));
        let row = world.row(unprocessed).unwrap();
        assert_eq!(
            (None, 0, false),
            (row.batch, row.pending.attempts, row.parked)
        );
        assert!(world.row(answered).is_none());
    }

    #[tokio::test]
    async fn should_spend_an_attempt_on_failed_requests_and_park_the_last_one() {
        let world = World::default();
        let rejected = world.add_pending("Ancient vase");
        let unanswered = world.add_pending("Brass lamp");
        world.state().work[1].pending.attempts = MAX_EMBEDDING_ATTEMPTS - 1;
        world.submitted(&[rejected, unanswered]);
        world.state().polls.push_back(ended(
            vec![(1, Err(EmbeddingError::InvalidResponse { reason: "test" }))],
            UnansweredEmbeddings::Failed,
        ));

        let report = handler(&world).execute(&system()).await.unwrap();

        assert_eq!(1, report.settlements.parked);
        let rejected = world.row(rejected).unwrap();
        assert_eq!(1, rejected.pending.attempts);
        // Requeued, then submitted again in the same round's new batch.
        assert!(!rejected.parked && rejected.batch.is_some());
        let unanswered = world.row(unanswered).unwrap();
        assert_eq!((true, None), (unanswered.parked, unanswered.batch));
    }

    #[tokio::test]
    async fn should_leave_a_batch_for_the_next_round_while_its_provider_is_unavailable() {
        let world = World::default();
        let work = world.add_pending("Ancient vase");
        world.submitted(&[work]);
        world
            .state()
            .polls
            .push_back(Err(EmbeddingError::DeadlineExceeded {
                deadline: Duration::from_secs(1),
            }));

        let report = handler(&world).execute(&system()).await.unwrap();

        assert_eq!(1, report.batches_unavailable);
        assert!(world.row(work).unwrap().batch.is_some());
        assert_eq!(0, report.submitted_embeddings);
    }

    #[tokio::test]
    async fn should_fail_every_item_once_when_the_provider_rejects_the_results() {
        let world = World::default();
        let work = world.add_pending("Ancient vase");
        world.submitted(&[work]);
        world
            .state()
            .polls
            .push_back(Err(EmbeddingError::InvalidResponse { reason: "test" }));

        let report = handler(&world).execute(&system()).await.unwrap();

        assert_eq!(1, report.batches_collected);
        assert_eq!(1, world.row(work).unwrap().pending.attempts);
    }

    #[tokio::test]
    async fn should_spend_an_attempt_when_the_guarded_write_fails() {
        let world = World::default();
        let work = world.add_pending("Ancient vase");
        world.submitted(&[work]);
        {
            let mut state = world.state();
            state.write_fails = true;
            state
                .polls
                .push_back(ended(vec![(1, Ok(vector()))], UnansweredEmbeddings::Failed));
        }

        let report = handler(&world).execute(&system()).await.unwrap();

        assert_eq!(1, report.settlements.write_failures);
        assert_eq!(1, world.row(work).unwrap().pending.attempts);
    }

    #[tokio::test]
    async fn should_park_a_blank_title_instead_of_submitting_it() {
        let world = World::default();
        let blank = world.add_pending("   ");
        let titled = world.add_pending("Brass lamp");

        let report = handler(&world).execute(&system()).await.unwrap();

        assert_eq!(
            (1, 1),
            (report.settlements.parked, report.submitted_embeddings)
        );
        assert!(world.row(blank).unwrap().parked);
        assert_eq!(
            Some(1),
            world
                .row(titled)
                .unwrap()
                .batch
                .map(|(_, position)| position)
        );
    }

    #[tokio::test]
    async fn should_return_work_to_the_queue_when_submission_fails() {
        let world = World::default();
        let work = world.add_pending("Ancient vase");
        world.state().submission_fails = true;

        let result = handler(&world).execute(&system()).await;

        assert!(matches!(result, Err(Error::SubmissionFailed { .. })));
        assert!(world.state().batches.is_empty());
        let row = world.row(work).unwrap();
        assert_eq!((None, 0), (row.batch, row.pending.attempts));
    }

    #[tokio::test]
    async fn should_leave_work_another_round_moved_on_untouched() {
        let world = World::default();
        let work = world.add_pending("Ancient vase");
        world.submitted(&[work]);
        world
            .state()
            .polls
            .push_back(ended(Vec::new(), UnansweredEmbeddings::Failed));
        let handler = handler(&world);
        let batch = handler.reader.submitted_batches().await.unwrap().remove(0);
        // Another round settled the batch after this one listed it.
        world.state().work[0].batch = None;
        world.state().work[0].parked = true;

        handler
            .collect(&batch, &mut ProductEmbeddingRoundReport::default())
            .await
            .unwrap();

        assert_eq!(0, world.row(work).unwrap().pending.attempts);
    }
}
