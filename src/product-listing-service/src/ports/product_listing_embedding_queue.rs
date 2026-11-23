//! Durable product embedding work between a listing's embedding revision and its committed
//! embedding: pending, submitted in a provider batch, or parked. One entry per listing holds its
//! latest embedding revision.

use application::error::BoxError;
use domain_primitives::event_id::EventId;
use product_listing_core::{
    description::Description, product_listing_id::ProductListingId, title::Title,
};
use std::time::Duration;
use url::Url;

/// The provider's handle of a submitted embedding batch.
pub use embedding::EmbeddingBatchId;

/// The work for one embedding revision of one listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProductEmbeddingWorkRef {
    pub product_listing_id: ProductListingId,
    pub source_event_id: EventId,
}

/// Work whose listing still holds its embedding revision, with that revision's document.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingProductEmbedding {
    pub work: ProductEmbeddingWorkRef,
    pub title: Title,
    /// A non-empty description.
    pub description: Option<Description>,
    /// The listing's first image.
    pub image_url: Option<Url>,
    /// Failed attempts so far.
    pub attempts: u16,
}

/// Work's place in a provider batch: the key of its request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductEmbeddingAssignment {
    pub work: ProductEmbeddingWorkRef,
    pub position: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BatchedProductEmbedding {
    pub position: u32,
    pub embedding: PendingProductEmbedding,
}

/// The local identity of a batch, assigned before the provider knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProductEmbeddingBatchKey(uuid::Uuid);

impl ProductEmbeddingBatchKey {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    pub fn from_uuid(value: uuid::Uuid) -> Self {
        Self(value)
    }

    pub fn as_uuid(&self) -> &uuid::Uuid {
        &self.0
    }
}

impl Default for ProductEmbeddingBatchKey {
    fn default() -> Self {
        Self::new()
    }
}

/// Where a round took the work it settles from. A settlement applies only while the work is
/// still there, so a round that read it before another round moved it on changes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductEmbeddingClaim {
    /// Assigned to this batch.
    Batch(ProductEmbeddingBatchKey),
    /// Pending: neither in a batch nor parked.
    Pending,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmittedProductEmbeddingBatch {
    pub key: ProductEmbeddingBatchKey,
    pub provider_batch_id: EmbeddingBatchId,
    /// Time since the provider accepted the batch.
    pub age: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductEmbeddingScheduleOutcome {
    Scheduled,
    /// The embedding of this revision is already committed.
    AlreadyEmbedded,
    /// The listing's embedding revision moved past this one, or the listing is gone.
    Superseded,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingEmbeddingQueueError {
    #[error("product embedding queue query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("product embedding queue persisted state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

/// Transaction-bound changes of the embedding work.
#[async_trait::async_trait]
pub trait ProductListingEmbeddingQueue: Send {
    /// Records the listing's embedding revision `source_event_id` as pending, replacing older
    /// work of the listing, when the listing still holds that revision and it is not embedded.
    async fn schedule(
        &mut self,
        work: ProductEmbeddingWorkRef,
    ) -> Result<ProductEmbeddingScheduleOutcome, ProductListingEmbeddingQueueError>;

    /// Deletes unsubmitted work whose listing no longer holds its revision or a title.
    async fn discard_superseded(&mut self) -> Result<u64, ProductListingEmbeddingQueueError>;

    /// Locks up to `limit` current pending work items for submission, oldest first, skipping
    /// work another transaction holds.
    async fn lock_pending(
        &mut self,
        limit: u32,
    ) -> Result<Vec<PendingProductEmbedding>, ProductListingEmbeddingQueueError>;

    /// Creates a batch the provider does not know yet and assigns the work to it.
    async fn create_batch(
        &mut self,
        key: ProductEmbeddingBatchKey,
        assignments: &[ProductEmbeddingAssignment],
    ) -> Result<(), ProductListingEmbeddingQueueError>;

    async fn record_submission(
        &mut self,
        key: ProductEmbeddingBatchKey,
        provider_batch_id: &EmbeddingBatchId,
    ) -> Result<(), ProductListingEmbeddingQueueError>;

    /// Returns the batch's unsettled work to the pending queue and deletes the batch.
    async fn close_batch(
        &mut self,
        key: ProductEmbeddingBatchKey,
    ) -> Result<(), ProductListingEmbeddingQueueError>;

    /// Closes batches created more than `older_than` ago whose submission was never recorded.
    async fn close_abandoned_submissions(
        &mut self,
        older_than: Duration,
    ) -> Result<u64, ProductListingEmbeddingQueueError>;

    /// Deletes the work once its revision is settled, while it is still `claim`ed. Newer work of
    /// the listing, or work another round already moved on, stays.
    async fn complete(
        &mut self,
        work: ProductEmbeddingWorkRef,
        claim: ProductEmbeddingClaim,
    ) -> Result<(), ProductListingEmbeddingQueueError>;

    /// Returns the work to the pending queue, while it is still `claim`ed.
    async fn requeue(
        &mut self,
        work: ProductEmbeddingWorkRef,
        claim: ProductEmbeddingClaim,
        attempts: u16,
    ) -> Result<(), ProductListingEmbeddingQueueError>;

    /// Keeps the work without retrying it, until a newer revision replaces it, while it is still
    /// `claim`ed.
    async fn park(
        &mut self,
        work: ProductEmbeddingWorkRef,
        claim: ProductEmbeddingClaim,
        attempts: u16,
    ) -> Result<(), ProductListingEmbeddingQueueError>;
}

pub trait ProductListingEmbeddingQueueFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductListingEmbeddingQueue + 'tx;
}

/// Reads of the embedding work outside a transaction.
#[async_trait::async_trait]
pub trait ProductListingEmbeddingQueueReader: Send + Sync {
    async fn submitted_batches(
        &self,
    ) -> Result<Vec<SubmittedProductEmbeddingBatch>, ProductListingEmbeddingQueueError>;

    /// The batch's work whose listing still holds its revision and title.
    async fn batch_items(
        &self,
        key: ProductEmbeddingBatchKey,
    ) -> Result<Vec<BatchedProductEmbedding>, ProductListingEmbeddingQueueError>;

    /// Up to `limit` current pending work items, oldest first.
    async fn pending(
        &self,
        limit: u32,
    ) -> Result<Vec<PendingProductEmbedding>, ProductListingEmbeddingQueueError>;
}
