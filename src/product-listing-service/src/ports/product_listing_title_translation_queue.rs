//! Durable title translation work between a listing's content revision and its committed
//! translations: pending, submitted in a provider batch, waiting for the refusal fallback, or
//! parked. One entry per listing holds its latest content revision.

use crate::ports::TitleTranslationBatchId;
use application::error::BoxError;
use domain_primitives::event_id::EventId;
use localization::Language;
use product_listing_core::{product_listing_id::ProductListingId, title::Title};
use std::time::Duration;

/// How the next attempt translates the title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleTranslationMode {
    /// With other titles of its source language in one request.
    Grouped,
    /// Alone in its request, after its group was refused or truncated.
    Single,
    /// Synchronously on the refusal fallback model, after the primary refused it alone.
    RefusalFallback,
}

/// The work for one content revision of one listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TitleTranslationWorkRef {
    pub product_listing_id: ProductListingId,
    pub source_event_id: EventId,
}

/// Work whose listing still holds its content revision, with that revision's title.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingTitleTranslation {
    pub work: TitleTranslationWorkRef,
    pub title: Title,
    pub source_language: Language,
    pub mode: TitleTranslationMode,
    /// Failed attempts so far.
    pub attempts: u16,
}

/// Work's place in a provider batch: the request (`group`) and the title's position in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TitleTranslationAssignment {
    pub work: TitleTranslationWorkRef,
    pub group: u32,
    pub position: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BatchedTitleTranslation {
    pub group: u32,
    pub position: u32,
    pub translation: PendingTitleTranslation,
}

/// The local identity of a batch, assigned before the provider knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TitleTranslationBatchKey(uuid::Uuid);

impl TitleTranslationBatchKey {
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

impl Default for TitleTranslationBatchKey {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmittedTitleTranslationBatch {
    pub key: TitleTranslationBatchKey,
    pub provider_batch_id: TitleTranslationBatchId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleTranslationScheduleOutcome {
    Scheduled,
    /// Translations of this content revision are already committed.
    AlreadyTranslated,
    /// The listing's content moved past this revision, or the listing is gone.
    Superseded,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingTitleTranslationQueueError {
    #[error("product title translation queue query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("product title translation queue persisted state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

/// Transaction-bound changes of the translation work.
#[async_trait::async_trait]
pub trait ProductListingTitleTranslationQueue: Send {
    /// Records the listing's content revision `source_event_id` as pending, replacing older work
    /// of the listing, when the listing still holds that revision and it is not yet translated.
    async fn schedule(
        &mut self,
        work: TitleTranslationWorkRef,
    ) -> Result<TitleTranslationScheduleOutcome, ProductListingTitleTranslationQueueError>;

    /// Deletes unsubmitted work whose listing no longer holds its revision or a title.
    async fn discard_superseded(&mut self)
    -> Result<u64, ProductListingTitleTranslationQueueError>;

    /// Locks up to `limit` pending `Grouped` or `Single` work items for submission, oldest first,
    /// skipping work another transaction holds.
    async fn lock_pending(
        &mut self,
        limit: u32,
    ) -> Result<Vec<PendingTitleTranslation>, ProductListingTitleTranslationQueueError>;

    /// Creates a batch the provider does not know yet and assigns the work to it.
    async fn create_batch(
        &mut self,
        key: TitleTranslationBatchKey,
        assignments: &[TitleTranslationAssignment],
    ) -> Result<(), ProductListingTitleTranslationQueueError>;

    async fn record_submission(
        &mut self,
        key: TitleTranslationBatchKey,
        provider_batch_id: &TitleTranslationBatchId,
    ) -> Result<(), ProductListingTitleTranslationQueueError>;

    /// Returns the batch's unsettled work to the pending queue and deletes the batch.
    async fn close_batch(
        &mut self,
        key: TitleTranslationBatchKey,
    ) -> Result<(), ProductListingTitleTranslationQueueError>;

    /// Closes batches created more than `older_than` ago whose submission was never recorded.
    async fn close_abandoned_submissions(
        &mut self,
        older_than: Duration,
    ) -> Result<u64, ProductListingTitleTranslationQueueError>;

    /// Deletes the work once its revision is settled. Newer work of the listing stays.
    async fn complete(
        &mut self,
        work: TitleTranslationWorkRef,
    ) -> Result<(), ProductListingTitleTranslationQueueError>;

    async fn requeue(
        &mut self,
        work: TitleTranslationWorkRef,
        mode: TitleTranslationMode,
        attempts: u16,
    ) -> Result<(), ProductListingTitleTranslationQueueError>;

    /// Keeps the work without retrying it, until a newer content revision replaces it.
    async fn park(
        &mut self,
        work: TitleTranslationWorkRef,
        attempts: u16,
    ) -> Result<(), ProductListingTitleTranslationQueueError>;
}

pub trait ProductListingTitleTranslationQueueFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl ProductListingTitleTranslationQueue + 'tx;
}

/// Reads of the translation work outside a transaction.
#[async_trait::async_trait]
pub trait ProductListingTitleTranslationQueueReader: Send + Sync {
    async fn submitted_batches(
        &self,
    ) -> Result<Vec<SubmittedTitleTranslationBatch>, ProductListingTitleTranslationQueueError>;

    /// The batch's work whose listing still holds its revision and title.
    async fn batch_items(
        &self,
        key: TitleTranslationBatchKey,
    ) -> Result<Vec<BatchedTitleTranslation>, ProductListingTitleTranslationQueueError>;

    /// Up to `limit` current work items waiting for the refusal fallback, oldest first.
    async fn refusal_fallbacks(
        &self,
        limit: u32,
    ) -> Result<Vec<PendingTitleTranslation>, ProductListingTitleTranslationQueueError>;
}
