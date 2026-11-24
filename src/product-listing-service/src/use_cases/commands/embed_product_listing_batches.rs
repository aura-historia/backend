//! One round of batched product embedding: collect ended provider batches and commit their
//! vectors under the embedding source-marker guard, then submit pending work as a new batch.
//!
//! Before submission each pending revision is compared with the input behind the listing's
//! current vector ([`embedding_input_change`]). A revision that does not change it meaningfully
//! is confirmed without a provider call; the vector keeps serving until a new one replaces it.
//!
//! Work is durable in the embedding queue, so a round that fails part-way leaves every
//! embedding pending, submitted or settled, and the next round continues from there.

use super::embedding_input_change::{EmbeddingInputChange, embedding_input_change};
use crate::ports::{
    BatchedProductEmbedding, ImageHash, PendingProductEmbedding, ProductEmbeddingAssignment,
    ProductEmbeddingBatchKey, ProductEmbeddingClaim, ProductEmbeddingWorkRef,
    ProductListingEmbeddingConfirmOutcome, ProductListingEmbeddingConfirmation,
    ProductListingEmbeddingQueue, ProductListingEmbeddingQueueError,
    ProductListingEmbeddingQueueFactory, ProductListingEmbeddingQueueReader,
    ProductListingEmbeddingWrite, ProductListingEmbeddingWriteError,
    ProductListingEmbeddingWriteOutcome, ProductListingEmbeddingWriter,
    ProductListingEmbeddingWriterFactory, StagedProductImage, SubmittedProductEmbeddingBatch,
};
use application::error::{BoxError, box_error};
use application::operation_context::{OperationAuthorizationError, OperationContext};
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use embedding::{
    EmbeddingBatchId, EmbeddingBatchPoll, EmbeddingError, EmbeddingImageHasher, EmbeddingImageUrl,
    EmbeddingText, EmbeddingVector, EndedEmbeddingBatch, ProductEmbeddingBatches,
    ProductEmbeddingRequest, SubmittedEmbeddingBatch, UnansweredEmbeddings,
};
use futures::{StreamExt, stream};
use std::collections::BTreeMap;
use std::time::Duration;

/// Embeddings one batch submits at most. Every request's image is fetched and staged while the
/// batch is submitted, which bounds a round's duration.
pub const MAX_EMBEDDINGS_PER_BATCH: u32 = 10_000;
/// A batch whose submission was not recorded within this time never reached the provider, or
/// reached it without its ID being kept; either way its work returns to the queue. It exceeds
/// the time comparing new first-image URLs and staging a full batch's images take, even when
/// every fetch runs into the fetcher's 10-second bound.
const ABANDONED_SUBMISSION_AGE: Duration = Duration::from_secs(2 * 60 * 60);
/// Failed attempts after which an embedding is parked until the listing's content changes.
pub const MAX_EMBEDDING_ATTEMPTS: u16 = 3;
/// The model and request format product embeddings are computed with. Bump it with any change of
/// either: an input recorded with another version counts as none, so every revision is embedded
/// again instead of confirmed, and a batch submitted with another version is submitted again
/// when collected (docs/durable-worker-runbook.md#forced-re-embedding).
pub const EMBEDDING_INPUT_VERSION: u16 = 1;
/// New first-image URLs fetched at once to compare their picture with the embedded one.
const IMAGE_COMPARISON_CONCURRENCY: usize = 32;

type Error = EmbedProductListingBatchesError;

#[derive(Debug, thiserror::Error)]
pub enum EmbedProductListingBatchesError {
    #[error("system principal required for product embedding batches")]
    SystemPrincipalRequired,
    #[error("product embedding queue failed")]
    QueueFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin product embedding transaction")]
    BeginTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to persist product embedding")]
    EmbeddingWriteFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to commit product embedding transaction")]
    CommitTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("product embedding batch submission failed")]
    SubmissionFailed {
        #[source]
        source: BoxError,
    },
}

impl EmbedProductListingBatchesError {
    /// An allowlisted label of the failed step, safe to log.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SystemPrincipalRequired => "system_principal_required",
            Self::QueueFailed { .. } => "queue",
            Self::BeginTransactionFailed { .. } => "write_begin",
            Self::EmbeddingWriteFailed { .. } => "embedding_write",
            Self::CommitTransactionFailed { .. } => "write_commit",
            Self::SubmissionFailed { .. } => "batch_submit",
        }
    }
}

impl From<OperationAuthorizationError> for EmbedProductListingBatchesError {
    fn from(_: OperationAuthorizationError) -> Self {
        Self::SystemPrincipalRequired
    }
}

impl From<ProductListingEmbeddingQueueError> for EmbedProductListingBatchesError {
    fn from(source: ProductListingEmbeddingQueueError) -> Self {
        Self::QueueFailed {
            source: box_error(source),
        }
    }
}

impl From<ProductListingEmbeddingWriteError> for EmbedProductListingBatchesError {
    fn from(source: ProductListingEmbeddingWriteError) -> Self {
        Self::EmbeddingWriteFailed {
            source: box_error(source),
        }
    }
}

/// How settled work ended, for a round's and a batch's log records.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProductEmbeddingSettlements {
    pub applied: usize,
    /// Revisions whose input did not change meaningfully: the current vector stands for them
    /// without a provider call.
    pub confirmed: usize,
    pub duplicate: usize,
    /// Results for a revision the listing moved past, or for a deleted listing.
    pub stale: usize,
    pub requeued: usize,
    pub parked: usize,
    /// Results whose guarded write failed; their work spent an attempt.
    pub write_failures: usize,
}

impl ProductEmbeddingSettlements {
    fn count(&mut self, outcome: ProductListingEmbeddingWriteOutcome) {
        match outcome {
            ProductListingEmbeddingWriteOutcome::Applied => self.applied += 1,
            ProductListingEmbeddingWriteOutcome::Duplicate => self.duplicate += 1,
            ProductListingEmbeddingWriteOutcome::Stale
            | ProductListingEmbeddingWriteOutcome::ProductListingNotFound => self.stale += 1,
        }
    }
}

impl std::ops::AddAssign for ProductEmbeddingSettlements {
    fn add_assign(&mut self, other: Self) {
        self.applied += other.applied;
        self.confirmed += other.confirmed;
        self.duplicate += other.duplicate;
        self.stale += other.stale;
        self.requeued += other.requeued;
        self.parked += other.parked;
        self.write_failures += other.write_failures;
    }
}

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
    ) -> Result<ProductEmbeddingRoundReport, EmbedProductListingBatchesError>;
}

pub struct EmbedProductListingBatchesHandler<R, B, H, U, Q, W> {
    reader: R,
    batches: B,
    images: H,
    unit_of_work: U,
    queue: Q,
    embeddings: W,
}

impl<R, B, H, U, Q, W> EmbedProductListingBatchesHandler<R, B, H, U, Q, W> {
    pub fn new(reader: R, batches: B, images: H, unit_of_work: U, queue: Q, embeddings: W) -> Self {
        Self {
            reader,
            batches,
            images,
            unit_of_work,
            queue,
            embeddings,
        }
    }
}

#[async_trait::async_trait]
impl<R, B, H, U, Q, W> EmbedProductListingBatchesUseCase
    for EmbedProductListingBatchesHandler<R, B, H, U, Q, W>
where
    R: ProductListingEmbeddingQueueReader,
    B: ProductEmbeddingBatches,
    H: EmbeddingImageHasher,
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

impl<R, B, H, U, Q, W> EmbedProductListingBatchesHandler<R, B, H, U, Q, W>
where
    R: ProductListingEmbeddingQueueReader,
    B: ProductEmbeddingBatches,
    H: EmbeddingImageHasher,
    U: UnitOfWork,
    Q: ProductListingEmbeddingQueueFactory<U::Tx>,
    W: ProductListingEmbeddingWriterFactory<U::Tx>,
{
    /// Settles every current item of an ended batch, then closes it. A batch whose results
    /// cannot be read now is left for the next round; one the provider rejected for good returns
    /// its items to the queue without spending attempts, because that says nothing about their
    /// documents. The backlog-age alarm catches a provider that keeps rejecting.
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
                    "Embedding batch results were rejected; its embeddings are submitted again"
                );
                EndedEmbeddingBatch {
                    results: Vec::new(),
                    unanswered: UnansweredEmbeddings::NotProcessed,
                }
            }
        };
        let ended = if batch.input_version == EMBEDDING_INPUT_VERSION {
            ended
        } else {
            tracing::warn!(
                event = "product.embedding.batch_outdated",
                provider_batch_id = %batch.provider_batch_id,
                input_version = batch.input_version,
                "Embedding batch used another input version; its embeddings are submitted again"
            );
            EndedEmbeddingBatch {
                results: Vec::new(),
                unanswered: UnansweredEmbeddings::NotProcessed,
            }
        };
        let items = self.reader.batch_items(batch.key).await?;
        let size = items.len();
        let answered = ended.results.len();
        let claim = ProductEmbeddingClaim::Batch(batch.key);
        let mut settlements = ProductEmbeddingSettlements::default();
        for (item, result) in settlements_of(items, ended) {
            self.settle(&item, claim, result, &mut settlements).await?;
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

    /// Confirms pending revisions whose input is unchanged and submits the others as one batch.
    async fn submit_pending(&self, report: &mut ProductEmbeddingRoundReport) -> Result<(), Error> {
        let key = ProductEmbeddingBatchKey::new();
        let triage = self.triage_pending(key, report).await?;
        for pending in &triage.unchanged {
            // A failed confirmation leaves the work pending for the next round.
            let _ = self
                .confirm(
                    pending,
                    ProductEmbeddingClaim::Pending,
                    &mut report.settlements,
                )
                .await;
        }
        let mut requests = triage.changed;
        for (comparison, matches) in self.compare_new_image_urls(triage.new_image_urls).await {
            let claim = ProductEmbeddingClaim::Batch(key);
            // A failed confirmation embeds the revision instead, so the batch is not held back.
            if !matches
                || self
                    .confirm(&comparison.pending, claim, &mut report.settlements)
                    .await
                    .is_err()
            {
                requests.push(comparison.request);
            }
        }
        if requests.is_empty() {
            if triage.batch_created {
                self.close_batch(key).await?;
            }
            return Ok(());
        }
        requests.sort_by_key(|request| request.key);
        match self
            .batches
            .submit(&key.as_uuid().to_string(), &requests)
            .await
        {
            Ok(submitted) => {
                self.record_submission(key, &submitted).await?;
                tracing::info!(
                    event = "product.embedding.batch.submitted",
                    provider_batch_id = %submitted.id,
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

    /// Locks the pending work and sorts it by how its revision changes the embedded input. Work
    /// that may need a provider call is assigned to the batch `key`, so no other round takes it
    /// while its images are compared; unchanged work stays pending until it is confirmed.
    async fn triage_pending(
        &self,
        key: ProductEmbeddingBatchKey,
        report: &mut ProductEmbeddingRoundReport,
    ) -> Result<PendingTriage, Error> {
        let mut tx = self.begin().await?;
        let mut triage = PendingTriage::default();
        {
            let mut queue = self.queue.in_transaction(&mut tx);
            report.superseded_discarded = queue.discard_superseded().await?;
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
                let change = embedding_input_change(&pending);
                if change == EmbeddingInputChange::Unchanged {
                    triage.unchanged.push(pending);
                    continue;
                }
                let position = u32::try_from(assignments.len() + 1).unwrap_or(u32::MAX);
                assignments.push(ProductEmbeddingAssignment {
                    work: pending.work,
                    position,
                });
                let request = document.request(position);
                match (change, document.image_url) {
                    (EmbeddingInputChange::NewImageUrl { embedded }, Some(image_url)) => {
                        triage.new_image_urls.push(ImageUrlComparison {
                            pending,
                            request,
                            image_url,
                            embedded,
                        });
                    }
                    _ => triage.changed.push(request),
                }
            }
            if !assignments.is_empty() {
                queue.create_batch(key, &assignments).await?;
                triage.batch_created = true;
            }
        }
        commit(tx).await?;
        Ok(triage)
    }

    /// Whether each new first-image URL serves the embedded picture. An image that cannot be
    /// fetched or decoded counts as changed, so its revision is embedded again.
    async fn compare_new_image_urls(
        &self,
        comparisons: Vec<ImageUrlComparison>,
    ) -> Vec<(ImageUrlComparison, bool)> {
        stream::iter(comparisons)
            .map(|comparison| async move {
                let matches = self
                    .images
                    .image_hash(&comparison.image_url)
                    .await
                    .is_some_and(|current| comparison.embedded.matches(&current));
                (comparison, matches)
            })
            .buffer_unordered(IMAGE_COMPARISON_CONCURRENCY)
            .collect()
            .await
    }

    /// Records that the listing's vector stands for the pending revision, while the work is still
    /// `claim`ed. Work whose embedded input changed since it was compared is decided again by a
    /// later round. A failure is logged and leaves the work where it is.
    async fn confirm(
        &self,
        pending: &PendingProductEmbedding,
        claim: ProductEmbeddingClaim,
        settlements: &mut ProductEmbeddingSettlements,
    ) -> Result<(), Error> {
        self.try_confirm(pending, claim, settlements)
            .await
            .inspect_err(|error| {
                tracing::warn!(
                    event = "product.embedding.confirm_failed",
                    product_listing_id = %pending.work.product_listing_id,
                    source_event_id = %pending.work.source_event_id,
                    error_kind = error.kind(),
                    "Embedding confirmation failed"
                );
            })
    }

    async fn try_confirm(
        &self,
        pending: &PendingProductEmbedding,
        claim: ProductEmbeddingClaim,
        settlements: &mut ProductEmbeddingSettlements,
    ) -> Result<(), Error> {
        let Some(embedded) = &pending.embedded else {
            return Ok(());
        };
        let work = pending.work;
        let mut tx = self.begin().await?;
        let outcome = self
            .embeddings
            .in_transaction(&mut tx)
            .confirm(&ProductListingEmbeddingConfirmation {
                product_listing_id: work.product_listing_id,
                source_event_id: work.source_event_id,
                embedded_source_event_id: embedded.source_event_id,
                image_url: pending.image_url.clone(),
            })
            .await?;
        {
            let mut queue = self.queue.in_transaction(&mut tx);
            match outcome {
                ProductListingEmbeddingConfirmOutcome::Confirmed => {
                    queue.complete(work, claim).await?;
                    settlements.confirmed += 1;
                }
                ProductListingEmbeddingConfirmOutcome::Superseded => {
                    queue.complete(work, claim).await?;
                    settlements.stale += 1;
                }
                ProductListingEmbeddingConfirmOutcome::EmbeddedInputChanged => {
                    queue.requeue(work, claim, pending.attempts).await?;
                    settlements.requeued += 1;
                }
            }
        }
        commit(tx).await
    }

    async fn close_abandoned_submissions(&self) -> Result<u64, Error> {
        let mut tx = self.begin().await?;
        let closed = self
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
        submitted: &SubmittedEmbeddingBatch,
    ) -> Result<(), Error> {
        let mut tx = self.begin().await?;
        self.queue
            .in_transaction(&mut tx)
            .record_submission(key, &submitted.id, &submitted.images)
            .await?;
        commit(tx).await
    }

    async fn close_batch(&self, key: ProductEmbeddingBatchKey) -> Result<(), Error> {
        let mut tx = self.begin().await?;
        self.queue.in_transaction(&mut tx).close_batch(key).await?;
        commit(tx).await
    }

    /// Commits the vector, or settles the failure that stands in for it, while the work is
    /// still `claim`ed. A write the listing's state rejects fails only this work.
    async fn settle(
        &self,
        item: &BatchedProductEmbedding,
        claim: ProductEmbeddingClaim,
        result: Result<EmbeddingVector, Retry>,
        settlements: &mut ProductEmbeddingSettlements,
    ) -> Result<(), Error> {
        let pending = &item.embedding;
        let work = pending.work;
        let retry = match result {
            Err(retry) => retry,
            Ok(vector) => match self.commit(work, claim, vector, item.staged_image).await {
                Ok(outcome) => {
                    settlements.count(outcome);
                    if outcome == ProductListingEmbeddingWriteOutcome::Applied {
                        tracing::info!(
                            event = "product.embedded",
                            actor_type = "system",
                            product_listing_id = %work.product_listing_id,
                            source_event_id = %work.source_event_id,
                            outcome = "success",
                        );
                    }
                    return Ok(());
                }
                Err(error @ Error::EmbeddingWriteFailed { .. }) => {
                    settlements.write_failures += 1;
                    tracing::warn!(
                        event = "product.embedding.write_failed",
                        product_listing_id = %work.product_listing_id,
                        source_event_id = %work.source_event_id,
                        error_kind = error.kind(),
                        "Embedding could not be written; it spends an attempt"
                    );
                    failed(pending)
                }
                Err(error) => return Err(error),
            },
        };
        self.retry(work, claim, retry, settlements).await
    }

    async fn retry(
        &self,
        work: ProductEmbeddingWorkRef,
        claim: ProductEmbeddingClaim,
        retry: Retry,
        settlements: &mut ProductEmbeddingSettlements,
    ) -> Result<(), Error> {
        let mut tx = self.begin().await?;
        match retry {
            Retry::Requeue(attempts) => {
                self.queue
                    .in_transaction(&mut tx)
                    .requeue(work, claim, attempts)
                    .await?;
                commit(tx).await?;
                settlements.requeued += 1;
            }
            Retry::Park(attempts) => {
                self.queue
                    .in_transaction(&mut tx)
                    .park(work, claim, attempts)
                    .await?;
                commit(tx).await?;
                settlements.parked += 1;
                tracing::warn!(
                    event = "product.embedding.parked",
                    product_listing_id = %work.product_listing_id,
                    source_event_id = %work.source_event_id,
                    attempts,
                    "Product embedding parked until the listing's content changes"
                );
            }
        }
        Ok(())
    }

    /// Writes the vector under the source-marker guard and completes the work together.
    async fn commit(
        &self,
        work: ProductEmbeddingWorkRef,
        claim: ProductEmbeddingClaim,
        vector: EmbeddingVector,
        image: Option<StagedProductImage>,
    ) -> Result<ProductListingEmbeddingWriteOutcome, Error> {
        let mut tx = self.begin().await?;
        let outcome = self
            .embeddings
            .in_transaction(&mut tx)
            .apply(&ProductListingEmbeddingWrite {
                product_listing_id: work.product_listing_id,
                source_event_id: work.source_event_id,
                enrichment_event_id: EventId::new(),
                embedding: vector.into_values(),
                image,
            })
            .await?;
        self.queue
            .in_transaction(&mut tx)
            .complete(work, claim)
            .await?;
        commit(tx).await?;
        Ok(outcome)
    }

    async fn begin(&self) -> Result<U::Tx, Error> {
        self.unit_of_work
            .begin()
            .await
            .map_err(|source| Error::BeginTransactionFailed {
                source: box_error(source),
            })
    }
}

/// The text and image one work item embeds.
struct EmbeddingDocument {
    title: EmbeddingText,
    additional_text: Option<EmbeddingText>,
    image_url: Option<EmbeddingImageUrl>,
}

/// Pending work sorted by how its revision changes the embedded input.
#[derive(Default)]
struct PendingTriage {
    unchanged: Vec<PendingProductEmbedding>,
    /// Requests of work assigned to the batch to embed it again.
    changed: Vec<ProductEmbeddingRequest>,
    /// Work assigned to the batch whose only change may be its first image's URL.
    new_image_urls: Vec<ImageUrlComparison>,
    batch_created: bool,
}

struct ImageUrlComparison {
    pending: PendingProductEmbedding,
    /// The request that embeds the revision when its picture changed.
    request: ProductEmbeddingRequest,
    image_url: EmbeddingImageUrl,
    embedded: ImageHash,
}

impl EmbeddingDocument {
    fn request(&self, key: u32) -> ProductEmbeddingRequest {
        ProductEmbeddingRequest {
            key,
            title: self.title.clone(),
            additional_text: self.additional_text.clone(),
            image_url: self.image_url.clone(),
        }
    }

    /// Fails for a document no provider can embed, such as a blank title.
    fn of(pending: &PendingProductEmbedding) -> Result<Self, EmbeddingError> {
        Ok(Self {
            title: EmbeddingText::new(pending.title.as_ref())?,
            additional_text: pending
                .description
                .as_ref()
                .filter(|description| !description.as_ref().is_empty())
                .map(|description| EmbeddingText::new(description.as_ref()))
                .transpose()?,
            image_url: pending
                .image_url
                .clone()
                .map(EmbeddingImageUrl::new)
                .transpose()?,
        })
    }
}

/// What becomes of work that did not commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Retry {
    /// Back to the pending queue with these failed attempts.
    Requeue(u16),
    /// No further attempt until the listing's content changes.
    Park(u16),
}

/// A provider that could not be asked or did not answer said nothing about the document, so
/// the work returns unchanged; any other failure spends an attempt.
fn after_error(pending: &PendingProductEmbedding, error: &EmbeddingError) -> Retry {
    if error.is_transient() {
        Retry::Requeue(pending.attempts)
    } else {
        failed(pending)
    }
}

/// One more failed attempt, or parked once the attempts are spent.
fn failed(pending: &PendingProductEmbedding) -> Retry {
    let attempts = pending.attempts.saturating_add(1);
    if attempts >= MAX_EMBEDDING_ATTEMPTS {
        Retry::Park(attempts)
    } else {
        Retry::Requeue(attempts)
    }
}

async fn commit<T: Transaction>(tx: T) -> Result<(), Error> {
    tx.commit()
        .await
        .map_err(|source| Error::CommitTransactionFailed {
            source: box_error(source),
        })
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
    use crate::ports::{EmbeddedImage, EmbeddedProductImage, EmbeddedProductInput};
    use product_listing_core::{
        description::Description, product_listing_id::ProductListingId, title::Title,
    };
    use url::Url;

    type Handler = EmbedProductListingBatchesHandler<World, World, World, World, World, World>;

    fn handler(world: &World) -> Handler {
        EmbedProductListingBatchesHandler::new(
            world.clone(),
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
    async fn should_submit_again_unspent_the_results_of_a_batch_of_another_input_version() {
        let world = World::default();
        let work = world.add_pending("Ancient vase");
        world.submitted(&[work]);
        {
            let mut state = world.state();
            state.batch_input_version = Some(EMBEDDING_INPUT_VERSION + 1);
            state
                .polls
                .push_back(ended(vec![(1, Ok(vector()))], UnansweredEmbeddings::Failed));
        }

        let report = handler(&world).execute(&system()).await.unwrap();

        assert_eq!(
            (0, 1, 1),
            (
                report.settlements.applied,
                report.settlements.requeued,
                report.submitted_embeddings
            )
        );
        assert!(world.state().writes.is_empty());
        assert_eq!(0, world.row(work).unwrap().pending.attempts);
    }

    #[tokio::test]
    async fn should_embed_instead_when_a_matching_image_cannot_be_confirmed() {
        let world = World::default();
        let work = add_embedded(&world, NEW_IMAGE_URL);
        {
            let mut state = world.state();
            state
                .image_hashes
                .insert(NEW_IMAGE_URL.to_owned(), image_hash(0));
            state.confirm_fails = true;
        }

        let report = handler(&world).execute(&system()).await.unwrap();

        assert_eq!(
            (0, 1),
            (report.settlements.confirmed, report.submitted_embeddings)
        );
        assert!(world.row(work).unwrap().batch.is_some());
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
    async fn should_return_every_item_unspent_when_the_provider_rejects_the_results() {
        let world = World::default();
        let work = world.add_pending("Ancient vase");
        world.submitted(&[work]);
        world
            .state()
            .polls
            .push_back(Err(EmbeddingError::InvalidResponse { reason: "test" }));

        let report = handler(&world).execute(&system()).await.unwrap();

        assert_eq!(1, report.batches_collected);
        assert_eq!(0, world.row(work).unwrap().pending.attempts);
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

    const IMAGE: &str = "https://example.test/vase.jpg?v=1";
    const NEW_IMAGE_URL: &str = "https://example.test/vase.jpg?v=2";

    fn image_hash(seed: u8) -> ImageHash {
        ImageHash::from_bytes(&[seed; ImageHash::BYTES]).unwrap()
    }

    /// Adds pending work whose listing's vector was computed from the same title, description and
    /// first image `IMAGE`, and whose revision has the first image `image_url`.
    fn add_embedded(world: &World, image_url: &str) -> ProductEmbeddingWorkRef {
        let work = world.add_pending("Ancient vase");
        let mut state = world.state();
        let row = &mut state.work.last_mut().unwrap().pending;
        row.image_url = Some(Url::parse(image_url).unwrap());
        row.embedded = Some(EmbeddedProductInput {
            source_event_id: EventId::new(),
            title: "Ancient vase".to_owned(),
            description: Some("Painted clay".to_owned()),
            image: Some(EmbeddedProductImage {
                url: Url::parse(IMAGE).unwrap(),
                hash: Some(image_hash(0)),
            }),
        });
        work
    }

    #[tokio::test]
    async fn should_confirm_an_unchanged_input_without_a_provider_call() {
        let world = World::default();
        let work = add_embedded(&world, IMAGE);

        let report = handler(&world).execute(&system()).await.unwrap();

        assert_eq!(
            (1, 0),
            (report.settlements.confirmed, report.submitted_embeddings)
        );
        let state = world.state();
        assert!(state.submitted.is_empty() && state.hashed_urls.is_empty());
        assert!(state.batches.is_empty(), "no batch is left behind");
        let [confirmation] = state.confirmations.as_slice() else {
            panic!("one confirmation");
        };
        assert_eq!(work.source_event_id, confirmation.source_event_id);
        assert_eq!(
            Some(IMAGE),
            confirmation.image_url.as_ref().map(Url::as_str)
        );
        drop(state);
        assert!(world.row(work).is_none());
    }

    #[tokio::test]
    async fn should_confirm_a_new_image_url_that_shows_the_embedded_picture() {
        let world = World::default();
        let work = add_embedded(&world, NEW_IMAGE_URL);
        // A rescaled copy: a few bits apart.
        let mut copy = [0; ImageHash::BYTES];
        copy[0] = 0b111;
        world.state().image_hashes.insert(
            NEW_IMAGE_URL.to_owned(),
            ImageHash::from_bytes(&copy).unwrap(),
        );

        let report = handler(&world).execute(&system()).await.unwrap();

        assert_eq!(1, report.settlements.confirmed);
        let state = world.state();
        assert_eq!(vec![NEW_IMAGE_URL.to_owned()], state.hashed_urls);
        assert!(state.submitted.is_empty());
        assert!(state.batches.is_empty(), "the emptied batch is closed");
        assert_eq!(
            Some(NEW_IMAGE_URL),
            state.confirmations[0].image_url.as_ref().map(Url::as_str)
        );
        drop(state);
        assert!(world.row(work).is_none());
    }

    #[tokio::test]
    async fn should_embed_again_when_a_new_image_url_shows_another_picture_or_none() {
        let world = World::default();
        let other_picture = add_embedded(&world, NEW_IMAGE_URL);
        let unfetchable = add_embedded(&world, "https://example.test/gone.jpg");
        let unchanged = add_embedded(&world, IMAGE);
        world
            .state()
            .image_hashes
            .insert(NEW_IMAGE_URL.to_owned(), image_hash(0xff));

        let report = handler(&world).execute(&system()).await.unwrap();

        assert_eq!(
            (1, 2),
            (report.settlements.confirmed, report.submitted_embeddings)
        );
        let state = world.state();
        let requests = &state.submitted[0].1;
        assert_eq!(
            vec![
                (1, Some(NEW_IMAGE_URL)),
                (2, Some("https://example.test/gone.jpg"))
            ],
            requests
                .iter()
                .map(|request| (
                    request.key,
                    request.image_url.as_ref().map(|url| url.as_url().as_str())
                ))
                .collect::<Vec<_>>()
        );
        drop(state);
        assert!(world.row(other_picture).unwrap().batch.is_some());
        assert!(world.row(unfetchable).unwrap().batch.is_some());
        assert!(world.row(unchanged).is_none());
    }

    #[tokio::test]
    async fn should_settle_a_confirmation_the_listing_moved_past_or_whose_vector_changed() {
        let world = World::default();
        let superseded = add_embedded(&world, IMAGE);
        world.state().confirm_outcome = Some(ProductListingEmbeddingConfirmOutcome::Superseded);

        let report = handler(&world).execute(&system()).await.unwrap();

        assert_eq!(
            (0, 1),
            (report.settlements.confirmed, report.settlements.stale)
        );
        assert!(world.row(superseded).is_none());

        let world = World::default();
        let decided_again = add_embedded(&world, NEW_IMAGE_URL);
        {
            let mut state = world.state();
            state.confirm_outcome =
                Some(ProductListingEmbeddingConfirmOutcome::EmbeddedInputChanged);
            state
                .image_hashes
                .insert(NEW_IMAGE_URL.to_owned(), image_hash(0));
        }

        let report = handler(&world).execute(&system()).await.unwrap();

        assert_eq!(1, report.settlements.requeued);
        let row = world.row(decided_again).unwrap();
        assert_eq!(
            (None, 0, false),
            (row.batch, row.pending.attempts, row.parked),
            "back to pending, without spending an attempt, for the next round to decide"
        );
    }

    #[tokio::test]
    async fn should_write_the_staged_image_with_its_vector() {
        let world = World::default();
        let with_image = world.add_pending("Ancient vase");
        let without_image = world.add_pending("Brass lamp");
        world.state().staged_images = vec![EmbeddedImage {
            key: 1,
            hash: Some(image_hash(7)),
        }];
        handler(&world).execute(&system()).await.unwrap();
        world.state().polls.push_back(ended(
            vec![(1, Ok(vector())), (2, Ok(vector()))],
            UnansweredEmbeddings::Failed,
        ));

        handler(&world).execute(&system()).await.unwrap();

        let state = world.state();
        let image_of = |work: ProductEmbeddingWorkRef| {
            state
                .writes
                .iter()
                .find(|write| write.source_event_id == work.source_event_id)
                .unwrap()
                .image
        };
        assert_eq!(
            Some(StagedProductImage {
                hash: Some(image_hash(7))
            }),
            image_of(with_image)
        );
        assert_eq!(None, image_of(without_image));
    }

    fn pending(attempts: u16) -> PendingProductEmbedding {
        PendingProductEmbedding {
            work: ProductEmbeddingWorkRef {
                product_listing_id: ProductListingId::new(),
                source_event_id: EventId::new(),
            },
            title: Title::from("Ancient vase"),
            description: Some(Description::from("Painted clay")),
            image_url: Some(Url::parse("https://example.test/vase.jpg").unwrap()),
            attempts,
            embedded: None,
        }
    }

    #[test]
    fn should_embed_title_description_and_first_image() {
        let document = EmbeddingDocument::of(&pending(0)).unwrap();

        assert_eq!("Ancient vase", document.title.as_str());
        assert_eq!(
            Some("Painted clay"),
            document.additional_text.as_ref().map(EmbeddingText::as_str)
        );
        assert_eq!(
            Some("https://example.test/vase.jpg"),
            document.image_url.as_ref().map(|url| url.as_url().as_str())
        );
    }

    #[test]
    fn should_leave_an_empty_description_out_of_the_document() {
        let mut pending = pending(0);
        pending.description = Some(Description::from("   "));

        assert!(
            EmbeddingDocument::of(&pending)
                .unwrap()
                .additional_text
                .is_none()
        );
    }

    #[test]
    fn should_park_after_the_last_attempt_and_spare_attempts_on_transient_failures() {
        let rejected = EmbeddingError::InvalidResponse { reason: "test" };
        let unavailable = EmbeddingError::DeadlineExceeded {
            deadline: std::time::Duration::from_secs(1),
        };

        assert_eq!(Retry::Requeue(1), after_error(&pending(0), &rejected));
        assert_eq!(
            Retry::Park(MAX_EMBEDDING_ATTEMPTS),
            after_error(&pending(MAX_EMBEDDING_ATTEMPTS - 1), &rejected)
        );
        assert_eq!(Retry::Requeue(2), after_error(&pending(2), &unavailable));
    }
}
