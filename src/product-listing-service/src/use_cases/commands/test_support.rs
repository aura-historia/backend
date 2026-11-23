//! An in-memory embedding queue, writer and provider for the embedding use cases' unit tests.
//! The queue follows the claim rules of the PostgreSQL adapter.

use crate::ports::{
    BatchedProductEmbedding, PendingProductEmbedding, ProductEmbeddingAssignment,
    ProductEmbeddingBatchKey, ProductEmbeddingClaim, ProductEmbeddingScheduleOutcome,
    ProductEmbeddingWorkRef, ProductListingEmbeddingQueue, ProductListingEmbeddingQueueError,
    ProductListingEmbeddingQueueFactory, ProductListingEmbeddingQueueReader,
    ProductListingEmbeddingWrite, ProductListingEmbeddingWriteError,
    ProductListingEmbeddingWriteOutcome, ProductListingEmbeddingWriter,
    ProductListingEmbeddingWriterFactory, SubmittedProductEmbeddingBatch,
};
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use application::transaction::{Transaction, TransactionError, UnitOfWork};
use domain_primitives::event_id::EventId;
use embedding::{
    EMBEDDING_DIMENSIONS, EmbeddingBatchId, EmbeddingBatchPoll, EmbeddingError, EmbeddingGenerator,
    EmbeddingImageUrl, EmbeddingText, EmbeddingVector, ProductEmbeddingBatches,
    ProductEmbeddingRequest,
};
use product_listing_core::{
    description::Description, product_listing_id::ProductListingId, title::Title,
};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

pub(crate) const PROVIDER_BATCH: &str = "projects/p/locations/eu/batchPredictionJobs/1";

#[derive(Debug, Clone)]
pub(crate) struct WorkRow {
    pub pending: PendingProductEmbedding,
    pub batch: Option<(ProductEmbeddingBatchKey, u32)>,
    pub parked: bool,
}

#[derive(Default)]
pub(crate) struct State {
    pub work: Vec<WorkRow>,
    pub batches: Vec<(ProductEmbeddingBatchKey, Option<EmbeddingBatchId>)>,
    pub writes: Vec<ProductListingEmbeddingWrite>,
    pub write_outcome: Option<ProductListingEmbeddingWriteOutcome>,
    pub write_fails: bool,
    pub submitted: Vec<(String, Vec<ProductEmbeddingRequest>)>,
    pub submission_fails: bool,
    pub polls: VecDeque<Result<EmbeddingBatchPoll, EmbeddingError>>,
    pub generated: Vec<String>,
    pub generation_error: Option<fn() -> EmbeddingError>,
}

/// Every port of the embedding use cases over one shared state.
#[derive(Clone, Default)]
pub(crate) struct World(Arc<Mutex<State>>);

impl World {
    pub fn state(&self) -> MutexGuard<'_, State> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Adds pending work for a new listing titled `title`.
    pub fn add_pending(&self, title: &str) -> ProductEmbeddingWorkRef {
        let work = ProductEmbeddingWorkRef {
            product_listing_id: ProductListingId::new(),
            source_event_id: EventId::new(),
        };
        self.state().work.push(WorkRow {
            pending: PendingProductEmbedding {
                work,
                title: Title::from(title),
                description: Some(Description::from("Painted clay")),
                image_url: None,
                attempts: 0,
            },
            batch: None,
            parked: false,
        });
        work
    }

    /// Adds a submitted batch holding `work` at positions 1, 2, …
    pub fn submitted(&self, work: &[ProductEmbeddingWorkRef]) -> ProductEmbeddingBatchKey {
        let key = ProductEmbeddingBatchKey::new();
        let mut state = self.state();
        state
            .batches
            .push((key, Some(EmbeddingBatchId::new(PROVIDER_BATCH).unwrap())));
        for (position, work) in (1..).zip(work) {
            state.row_mut(*work).unwrap().batch = Some((key, position));
        }
        key
    }

    pub fn row(&self, work: ProductEmbeddingWorkRef) -> Option<WorkRow> {
        self.state()
            .work
            .iter()
            .find(|row| row.pending.work == work)
            .cloned()
    }
}

impl State {
    fn row_mut(&mut self, work: ProductEmbeddingWorkRef) -> Option<&mut WorkRow> {
        self.work.iter_mut().find(|row| row.pending.work == work)
    }

    /// The row of `work` while `claim` still holds it, as the adapter's claim guard matches.
    fn claimed(
        &mut self,
        work: ProductEmbeddingWorkRef,
        claim: ProductEmbeddingClaim,
    ) -> Option<&mut WorkRow> {
        self.row_mut(work).filter(|row| match claim {
            ProductEmbeddingClaim::Batch(key) => row.batch.map(|(batch, _)| batch) == Some(key),
            ProductEmbeddingClaim::Pending => row.batch.is_none() && !row.parked,
        })
    }
}

pub(crate) fn vector() -> EmbeddingVector {
    EmbeddingVector::try_new(vec![1.0; EMBEDDING_DIMENSIONS]).unwrap()
}

pub(crate) fn system() -> OperationContext {
    OperationContext {
        principal: Principal::System,
        request_id: RequestId::new("request"),
        correlation_id: CorrelationId::new("correlation"),
    }
}

pub(crate) struct Tx;

#[async_trait::async_trait]
impl Transaction for Tx {
    async fn commit(self) -> Result<(), TransactionError> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl UnitOfWork for World {
    type Tx = Tx;
    async fn begin(&self) -> Result<Tx, TransactionError> {
        Ok(Tx)
    }
}

type QueueResult<T> = Result<T, ProductListingEmbeddingQueueError>;

impl ProductListingEmbeddingQueueFactory<Tx> for World {
    fn in_transaction<'tx>(&'tx self, _: &'tx mut Tx) -> impl ProductListingEmbeddingQueue + 'tx {
        self.clone()
    }
}

#[async_trait::async_trait]
impl ProductListingEmbeddingQueue for World {
    async fn schedule(
        &mut self,
        _: ProductEmbeddingWorkRef,
    ) -> QueueResult<ProductEmbeddingScheduleOutcome> {
        unreachable!("rounds and sweeps never schedule")
    }

    async fn discard_superseded(&mut self) -> QueueResult<u64> {
        Ok(0)
    }

    async fn lock_pending(&mut self, limit: u32) -> QueueResult<Vec<PendingProductEmbedding>> {
        ProductListingEmbeddingQueueReader::pending(self, limit).await
    }

    async fn create_batch(
        &mut self,
        key: ProductEmbeddingBatchKey,
        assignments: &[ProductEmbeddingAssignment],
    ) -> QueueResult<()> {
        let mut state = self.state();
        state.batches.push((key, None));
        for assignment in assignments {
            state.row_mut(assignment.work).unwrap().batch = Some((key, assignment.position));
        }
        Ok(())
    }

    async fn record_submission(
        &mut self,
        key: ProductEmbeddingBatchKey,
        provider_batch_id: &EmbeddingBatchId,
    ) -> QueueResult<()> {
        let mut state = self.state();
        let batch = state.batches.iter_mut().find(|(batch, _)| *batch == key);
        batch.unwrap().1 = Some(provider_batch_id.clone());
        Ok(())
    }

    async fn close_batch(&mut self, key: ProductEmbeddingBatchKey) -> QueueResult<()> {
        let mut state = self.state();
        for row in &mut state.work {
            if row.batch.map(|(batch, _)| batch) == Some(key) {
                row.batch = None;
            }
        }
        state.batches.retain(|(batch, _)| *batch != key);
        Ok(())
    }

    async fn close_abandoned_submissions(&mut self, _: Duration) -> QueueResult<u64> {
        Ok(0)
    }

    async fn complete(
        &mut self,
        work: ProductEmbeddingWorkRef,
        claim: ProductEmbeddingClaim,
    ) -> QueueResult<()> {
        let mut state = self.state();
        if state.claimed(work, claim).is_some() {
            state.work.retain(|row| row.pending.work != work);
        }
        Ok(())
    }

    async fn requeue(
        &mut self,
        work: ProductEmbeddingWorkRef,
        claim: ProductEmbeddingClaim,
        attempts: u16,
    ) -> QueueResult<()> {
        if let Some(row) = self.state().claimed(work, claim) {
            row.batch = None;
            row.pending.attempts = attempts;
        }
        Ok(())
    }

    async fn park(
        &mut self,
        work: ProductEmbeddingWorkRef,
        claim: ProductEmbeddingClaim,
        attempts: u16,
    ) -> QueueResult<()> {
        if let Some(row) = self.state().claimed(work, claim) {
            row.batch = None;
            row.parked = true;
            row.pending.attempts = attempts;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl ProductListingEmbeddingQueueReader for World {
    async fn submitted_batches(&self) -> QueueResult<Vec<SubmittedProductEmbeddingBatch>> {
        Ok(self
            .state()
            .batches
            .iter()
            .filter_map(|(key, provider_batch_id)| {
                Some(SubmittedProductEmbeddingBatch {
                    key: *key,
                    provider_batch_id: provider_batch_id.clone()?,
                    age: Duration::from_secs(600),
                })
            })
            .collect())
    }

    async fn batch_items(
        &self,
        key: ProductEmbeddingBatchKey,
    ) -> QueueResult<Vec<BatchedProductEmbedding>> {
        Ok(self
            .state()
            .work
            .iter()
            .filter_map(|row| match row.batch {
                Some((batch, position)) if batch == key => Some(BatchedProductEmbedding {
                    position,
                    embedding: row.pending.clone(),
                }),
                _ => None,
            })
            .collect())
    }

    async fn pending(&self, limit: u32) -> QueueResult<Vec<PendingProductEmbedding>> {
        Ok(self
            .state()
            .work
            .iter()
            .filter(|row| row.batch.is_none() && !row.parked)
            .take(limit as usize)
            .map(|row| row.pending.clone())
            .collect())
    }
}

impl ProductListingEmbeddingWriterFactory<Tx> for World {
    fn in_transaction<'tx>(&'tx self, _: &'tx mut Tx) -> impl ProductListingEmbeddingWriter + 'tx {
        self.clone()
    }
}

#[async_trait::async_trait]
impl ProductListingEmbeddingWriter for World {
    async fn apply(
        &mut self,
        write: &ProductListingEmbeddingWrite,
    ) -> Result<ProductListingEmbeddingWriteOutcome, ProductListingEmbeddingWriteError> {
        let mut state = self.state();
        if state.write_fails {
            return Err(ProductListingEmbeddingWriteError::WriteFailed {
                source: Box::new(std::io::Error::other("rejected")),
            });
        }
        state.writes.push(write.clone());
        Ok(state
            .write_outcome
            .unwrap_or(ProductListingEmbeddingWriteOutcome::Applied))
    }
}

#[async_trait::async_trait]
impl ProductEmbeddingBatches for World {
    async fn submit(
        &self,
        label: &str,
        requests: &[ProductEmbeddingRequest],
    ) -> Result<EmbeddingBatchId, EmbeddingError> {
        let mut state = self.state();
        state.submitted.push((label.to_owned(), requests.to_vec()));
        if state.submission_fails {
            return Err(EmbeddingError::InvalidResponse { reason: "test" });
        }
        EmbeddingBatchId::new(PROVIDER_BATCH)
    }

    async fn poll(&self, _: &EmbeddingBatchId) -> Result<EmbeddingBatchPoll, EmbeddingError> {
        self.state()
            .polls
            .pop_front()
            .unwrap_or(Ok(EmbeddingBatchPoll::InProgress))
    }
}

#[async_trait::async_trait]
impl EmbeddingGenerator for World {
    async fn embed_product(
        &self,
        title: &EmbeddingText,
        _: Option<&EmbeddingText>,
        _: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        let mut state = self.state();
        state.generated.push(title.as_str().to_owned());
        match state.generation_error {
            Some(error) => Err(error()),
            None => Ok(vector()),
        }
    }

    async fn embed_search_query(
        &self,
        _: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        unreachable!("sweeps embed products only")
    }
}
