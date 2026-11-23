//! What the [batch rounds](super::embed_product_listing_batches) and the
//! [sweep](super::sweep_product_listing_embeddings) share: the document a work item embeds and
//! how a result settles its work. A vector commits under the embedding source-marker guard
//! together with the work's completion; a failure returns the work to the queue or parks it.

use crate::ports::{
    PendingProductEmbedding, ProductEmbeddingClaim, ProductEmbeddingWorkRef,
    ProductListingEmbeddingQueue, ProductListingEmbeddingQueueError,
    ProductListingEmbeddingQueueFactory, ProductListingEmbeddingWrite,
    ProductListingEmbeddingWriteError, ProductListingEmbeddingWriteOutcome,
    ProductListingEmbeddingWriter, ProductListingEmbeddingWriterFactory,
};
use application::error::{BoxError, box_error};
use application::operation_context::OperationAuthorizationError;
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use embedding::{EmbeddingError, EmbeddingImageUrl, EmbeddingText, EmbeddingVector};

/// Failed attempts after which an embedding is parked until the listing's content changes.
pub const MAX_EMBEDDING_ATTEMPTS: u16 = 3;

#[derive(Debug, thiserror::Error)]
pub enum ProductEmbeddingWorkError {
    #[error("system principal required for product embedding work")]
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

impl ProductEmbeddingWorkError {
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

impl From<OperationAuthorizationError> for ProductEmbeddingWorkError {
    fn from(_: OperationAuthorizationError) -> Self {
        Self::SystemPrincipalRequired
    }
}

impl From<ProductListingEmbeddingQueueError> for ProductEmbeddingWorkError {
    fn from(source: ProductListingEmbeddingQueueError) -> Self {
        Self::QueueFailed {
            source: box_error(source),
        }
    }
}

impl From<ProductListingEmbeddingWriteError> for ProductEmbeddingWorkError {
    fn from(source: ProductListingEmbeddingWriteError) -> Self {
        Self::EmbeddingWriteFailed {
            source: box_error(source),
        }
    }
}

type Error = ProductEmbeddingWorkError;

/// How settled work ended, for the log records of rounds and sweeps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProductEmbeddingSettlements {
    pub applied: usize,
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
        self.duplicate += other.duplicate;
        self.stale += other.stale;
        self.requeued += other.requeued;
        self.parked += other.parked;
        self.write_failures += other.write_failures;
    }
}

/// The text and image one work item embeds.
pub(crate) struct EmbeddingDocument {
    pub title: EmbeddingText,
    pub additional_text: Option<EmbeddingText>,
    pub image_url: Option<EmbeddingImageUrl>,
}

impl EmbeddingDocument {
    /// Fails for a document no provider can embed, such as a blank title.
    pub(crate) fn of(pending: &PendingProductEmbedding) -> Result<Self, EmbeddingError> {
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
pub(crate) enum Retry {
    /// Back to the pending queue with these failed attempts.
    Requeue(u16),
    /// No further attempt until the listing's content changes.
    Park(u16),
}

/// A provider that could not be asked or did not answer said nothing about the document, so
/// the work returns unchanged; any other failure spends an attempt.
pub(crate) fn after_error(pending: &PendingProductEmbedding, error: &EmbeddingError) -> Retry {
    if error.is_transient() {
        Retry::Requeue(pending.attempts)
    } else {
        failed(pending)
    }
}

/// One more failed attempt, or parked once the attempts are spent.
pub(crate) fn failed(pending: &PendingProductEmbedding) -> Retry {
    let attempts = pending.attempts.saturating_add(1);
    if attempts >= MAX_EMBEDDING_ATTEMPTS {
        Retry::Park(attempts)
    } else {
        Retry::Requeue(attempts)
    }
}

/// Settles work in transactions of its own: commits vectors and moves unsettled work on.
pub(crate) struct ProductEmbeddingSettler<U, Q, W> {
    pub unit_of_work: U,
    pub queue: Q,
    pub embeddings: W,
}

impl<U, Q, W> ProductEmbeddingSettler<U, Q, W>
where
    U: UnitOfWork,
    Q: ProductListingEmbeddingQueueFactory<U::Tx>,
    W: ProductListingEmbeddingWriterFactory<U::Tx>,
{
    /// Commits the vector, or settles the failure that stands in for it, while the work is
    /// still `claim`ed. A write the listing's state rejects fails only this work.
    pub(crate) async fn settle(
        &self,
        pending: &PendingProductEmbedding,
        claim: ProductEmbeddingClaim,
        result: Result<EmbeddingVector, Retry>,
        settlements: &mut ProductEmbeddingSettlements,
    ) -> Result<(), Error> {
        let work = pending.work;
        let retry = match result {
            Err(retry) => retry,
            Ok(vector) => match self.commit(work, claim, vector).await {
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

    pub(crate) async fn retry(
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
            })
            .await?;
        self.queue
            .in_transaction(&mut tx)
            .complete(work, claim)
            .await?;
        commit(tx).await?;
        Ok(outcome)
    }

    pub(crate) async fn begin(&self) -> Result<U::Tx, Error> {
        self.unit_of_work
            .begin()
            .await
            .map_err(|source| Error::BeginTransactionFailed {
                source: box_error(source),
            })
    }
}

pub(crate) async fn commit<T: Transaction>(tx: T) -> Result<(), Error> {
    tx.commit()
        .await
        .map_err(|source| Error::CommitTransactionFailed {
            source: box_error(source),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use product_listing_core::{
        description::Description, product_listing_id::ProductListingId, title::Title,
    };
    use url::Url;

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
