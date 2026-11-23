//! The sweep before the daily saved-search match: embeds every unsettled embedding
//! synchronously, at the synchronous price, so the match sees it. That includes work inside a
//! batch that has not ended; its batch result then settles as a duplicate.

use super::product_embedding_work::{
    EmbeddingDocument, ProductEmbeddingSettlements, ProductEmbeddingSettler,
    ProductEmbeddingWorkError, Retry, failed,
};
use crate::ports::{
    ClaimedProductEmbedding, ProductEmbeddingClaim, ProductListingEmbeddingQueueFactory,
    ProductListingEmbeddingQueueReader, ProductListingEmbeddingWriterFactory,
};
use application::operation_context::OperationContext;
use application::transaction::UnitOfWork;
use embedding::{EmbeddingError, EmbeddingGenerator, EmbeddingVector};
use futures::StreamExt;
use std::time::Duration;

/// Unsettled embeddings one sweep takes at most. It exceeds a day's arrivals, so only a batch
/// path broken for days leaves work to the next sweep.
pub const MAX_SWEPT_EMBEDDINGS: u32 = 20_000;
/// Synchronous embeddings in flight at once.
const SWEEP_CONCURRENCY: usize = 16;
/// The sweep stops settling after this long, so it ends before the match an hour after it starts.
const MAX_SWEEP_DURATION: Duration = Duration::from_secs(45 * 60);

type Error = ProductEmbeddingWorkError;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProductEmbeddingSweepReport {
    pub swept: usize,
    pub settlements: ProductEmbeddingSettlements,
    /// Embeddings the provider could not be asked for or did not answer; they stay where they were.
    pub unavailable: usize,
    /// Results that could not be settled for a PostgreSQL failure; their work stays where it was.
    pub settlement_failures: usize,
    /// Embeddings left unsettled because the sweep ran out of time.
    pub out_of_time: usize,
}

#[async_trait::async_trait]
pub trait SweepProductListingEmbeddingsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
    ) -> Result<ProductEmbeddingSweepReport, ProductEmbeddingWorkError>;
}

pub struct SweepProductListingEmbeddingsHandler<R, G, U, Q, W> {
    reader: R,
    generator: G,
    settler: ProductEmbeddingSettler<U, Q, W>,
    max_duration: Duration,
}

impl<R, G, U, Q, W> SweepProductListingEmbeddingsHandler<R, G, U, Q, W> {
    pub fn new(reader: R, generator: G, unit_of_work: U, queue: Q, embeddings: W) -> Self {
        Self {
            reader,
            generator,
            settler: ProductEmbeddingSettler {
                unit_of_work,
                queue,
                embeddings,
            },
            max_duration: MAX_SWEEP_DURATION,
        }
    }
}

/// What the sweep does with one embedding's outcome.
enum Outcome {
    Settle(Result<EmbeddingVector, Retry>),
    /// The provider could not be asked or did not answer.
    Unavailable,
    /// Work in a batch that failed here stays for its batch to decide.
    LeaveToBatch,
}

#[async_trait::async_trait]
impl<R, G, U, Q, W> SweepProductListingEmbeddingsUseCase
    for SweepProductListingEmbeddingsHandler<R, G, U, Q, W>
where
    R: ProductListingEmbeddingQueueReader,
    G: EmbeddingGenerator,
    U: UnitOfWork,
    Q: ProductListingEmbeddingQueueFactory<U::Tx>,
    W: ProductListingEmbeddingWriterFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "sweep_product_embeddings",
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
    ) -> Result<ProductEmbeddingSweepReport, Error> {
        context.require().system().authorize::<Error>()?;
        let deadline = tokio::time::Instant::now() + self.max_duration;
        let unsettled = self.reader.unsettled(MAX_SWEPT_EMBEDDINGS).await?;
        let mut report = ProductEmbeddingSweepReport {
            swept: unsettled.len(),
            ..Default::default()
        };
        // Collected first: a lazily mapped stream would not prove itself `Send` here.
        let embeddings = unsettled
            .into_iter()
            .map(|claimed| async move {
                let outcome = self.embed(&claimed).await;
                (claimed, outcome)
            })
            .collect::<Vec<_>>();
        let mut embedded = futures::stream::iter(embeddings).buffer_unordered(SWEEP_CONCURRENCY);
        let mut settled = 0;
        loop {
            let next = tokio::time::timeout_at(deadline, embedded.next()).await;
            let Ok(Some((claimed, outcome))) = next else {
                report.out_of_time = report.swept - settled;
                break;
            };
            settled += 1;
            let result = match outcome {
                Outcome::Settle(result) => result,
                Outcome::Unavailable => {
                    report.unavailable += 1;
                    continue;
                }
                Outcome::LeaveToBatch => continue,
            };
            // One failed settlement must not cost the rest of the sweep its paid embeddings.
            if let Err(error) = self
                .settler
                .settle(
                    &claimed.embedding,
                    claimed.claim,
                    result,
                    &mut report.settlements,
                )
                .await
            {
                report.settlement_failures += 1;
                tracing::warn!(
                    event = "product.embedding.sweep.settlement_failed",
                    product_listing_id = %claimed.embedding.work.product_listing_id,
                    error_kind = error.kind(),
                    "Swept embedding could not be settled; its work stays where it was"
                );
            }
        }
        Ok(report)
    }
}

impl<R, G, U, Q, W> SweepProductListingEmbeddingsHandler<R, G, U, Q, W>
where
    G: EmbeddingGenerator,
{
    async fn embed(&self, claimed: &ClaimedProductEmbedding) -> Outcome {
        let pending = &claimed.embedding;
        let in_batch = matches!(claimed.claim, ProductEmbeddingClaim::Batch(_));
        let Ok(document) = EmbeddingDocument::of(pending) else {
            return if in_batch {
                Outcome::LeaveToBatch
            } else {
                Outcome::Settle(Err(Retry::Park(pending.attempts.saturating_add(1))))
            };
        };
        match self
            .generator
            .embed_product(
                &document.title,
                document.additional_text.as_ref(),
                document.image_url.as_ref(),
            )
            .await
        {
            Ok(vector) => Outcome::Settle(Ok(vector)),
            Err(error) if EmbeddingError::is_transient(&error) => Outcome::Unavailable,
            Err(_) if in_batch => Outcome::LeaveToBatch,
            Err(_) => Outcome::Settle(Err(failed(pending))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{World, system};
    use super::*;
    use embedding::EmbeddingError;

    type Handler = SweepProductListingEmbeddingsHandler<World, World, World, World, World>;

    fn handler(world: &World) -> Handler {
        SweepProductListingEmbeddingsHandler::new(
            world.clone(),
            world.clone(),
            world.clone(),
            world.clone(),
            world.clone(),
        )
    }

    #[tokio::test]
    async fn should_embed_and_commit_pending_and_batched_work_but_not_parked_work() {
        let world = World::default();
        let pending = world.add_pending("Ancient vase");
        let batched = world.add_pending("Brass lamp");
        let parked = world.add_pending("Oak chair");
        world.submitted(&[batched]);
        world.state().work[2].parked = true;

        let report = handler(&world).execute(&system()).await.unwrap();

        assert_eq!((2, 2), (report.swept, report.settlements.applied));
        assert!(world.row(pending).is_none());
        assert!(world.row(batched).is_none());
        assert!(world.row(parked).is_some());
    }

    #[tokio::test]
    async fn should_leave_work_for_the_batches_while_the_provider_is_unavailable() {
        let world = World::default();
        let work = world.add_pending("Ancient vase");
        world.state().generation_error = Some(|| EmbeddingError::DeadlineExceeded {
            deadline: std::time::Duration::from_secs(1),
        });

        let report = handler(&world).execute(&system()).await.unwrap();

        assert_eq!((1, 0), (report.unavailable, report.settlements.requeued));
        let row = world.row(work).unwrap();
        assert_eq!((0, false), (row.pending.attempts, row.parked));
    }

    #[tokio::test]
    async fn should_leave_rejected_batched_work_to_its_batch() {
        let world = World::default();
        let work = world.add_pending("Ancient vase");
        let key = world.submitted(&[work]);
        world.state().generation_error =
            Some(|| EmbeddingError::InvalidResponse { reason: "test" });

        handler(&world).execute(&system()).await.unwrap();

        let row = world.row(work).unwrap();
        assert_eq!((Some((key, 1)), 0), (row.batch, row.pending.attempts));
    }

    #[tokio::test]
    async fn should_keep_sweeping_after_a_failed_settlement() {
        let world = World::default();
        world.add_pending("Ancient vase");
        world.add_pending("Brass lamp");
        world.state().begin_fails_once = true;

        let report = handler(&world).execute(&system()).await.unwrap();

        assert_eq!((2, 1), (report.swept, report.settlement_failures));
        assert_eq!(1, report.settlements.applied);
    }

    #[tokio::test]
    async fn should_stop_settling_when_out_of_time() {
        let world = World::default();
        let work = world.add_pending("Ancient vase");
        world.state().generation_stalls = true;
        let mut handler = handler(&world);
        handler.max_duration = Duration::from_millis(10);

        let report = handler.execute(&system()).await.unwrap();

        assert_eq!(1, report.out_of_time);
        assert!(world.row(work).is_some());
    }

    #[tokio::test]
    async fn should_spend_an_attempt_on_a_rejected_document() {
        let world = World::default();
        let work = world.add_pending("Ancient vase");
        world.state().generation_error =
            Some(|| EmbeddingError::InvalidResponse { reason: "test" });

        handler(&world).execute(&system()).await.unwrap();

        assert_eq!(1, world.row(work).unwrap().pending.attempts);
    }

    #[tokio::test]
    async fn should_not_commit_work_a_round_assigned_meanwhile() {
        let world = World::default();
        let work = world.add_pending("Ancient vase");
        let handler = handler(&world);
        let pending = handler
            .reader
            .unsettled(10)
            .await
            .unwrap()
            .remove(0)
            .embedding;
        // A round submits the work after the sweep read it.
        let key = world.submitted(&[work]);

        handler
            .settler
            .settle(
                &pending,
                ProductEmbeddingClaim::Pending,
                Err(Retry::Requeue(1)),
                &mut ProductEmbeddingSettlements::default(),
            )
            .await
            .unwrap();

        let row = world.row(work).unwrap();
        assert_eq!((Some((key, 1)), 0), (row.batch, row.pending.attempts));
    }
}
