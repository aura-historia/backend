//! The sweep before the daily saved-search match: embeds pending work synchronously, at the
//! synchronous price, so the match sees it. Work already in a batch stays there.

use super::product_embedding_work::{
    EmbeddingDocument, ProductEmbeddingSettlements, ProductEmbeddingSettler,
    ProductEmbeddingWorkError, Retry, after_error,
};
use crate::ports::{
    PendingProductEmbedding, ProductEmbeddingClaim, ProductListingEmbeddingQueueFactory,
    ProductListingEmbeddingQueueReader, ProductListingEmbeddingWriterFactory,
};
use application::operation_context::OperationContext;
use application::transaction::UnitOfWork;
use embedding::{EmbeddingGenerator, EmbeddingVector};
use futures::StreamExt;

/// Pending embeddings one sweep takes at most. It exceeds a day's arrivals, so only a batch
/// path broken for days leaves work to the next sweep.
pub const MAX_SWEPT_EMBEDDINGS: u32 = 20_000;
/// Synchronous embeddings in flight at once.
const SWEEP_CONCURRENCY: usize = 16;

type Error = ProductEmbeddingWorkError;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProductEmbeddingSweepReport {
    pub swept: usize,
    pub settlements: ProductEmbeddingSettlements,
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
        }
    }
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
        let pending = self.reader.pending(MAX_SWEPT_EMBEDDINGS).await?;
        let mut report = ProductEmbeddingSweepReport {
            swept: pending.len(),
            ..Default::default()
        };
        // Collected first: a lazily mapped stream would not prove itself `Send` here.
        let embeddings = pending
            .into_iter()
            .map(|pending| async move {
                let result = self.embed(&pending).await;
                (pending, result)
            })
            .collect::<Vec<_>>();
        let mut embedded = futures::stream::iter(embeddings).buffer_unordered(SWEEP_CONCURRENCY);
        while let Some((pending, result)) = embedded.next().await {
            self.settler
                .settle(
                    &pending,
                    ProductEmbeddingClaim::Pending,
                    result,
                    &mut report.settlements,
                )
                .await?;
        }
        Ok(report)
    }
}

impl<R, G, U, Q, W> SweepProductListingEmbeddingsHandler<R, G, U, Q, W>
where
    G: EmbeddingGenerator,
{
    /// A document no provider embeds parks at once; a provider failure settles as in a batch.
    async fn embed(&self, pending: &PendingProductEmbedding) -> Result<EmbeddingVector, Retry> {
        let document = EmbeddingDocument::of(pending)
            .map_err(|_| Retry::Park(pending.attempts.saturating_add(1)))?;
        self.generator
            .embed_product(
                &document.title,
                document.additional_text.as_ref(),
                document.image_url.as_ref(),
            )
            .await
            .map_err(|error| after_error(pending, &error))
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
    async fn should_embed_and_commit_pending_work_but_leave_batched_work_alone() {
        let world = World::default();
        let pending = world.add_pending("Ancient vase");
        let batched = world.add_pending("Brass lamp");
        world.submitted(&[batched]);

        let report = handler(&world).execute(&system()).await.unwrap();

        assert_eq!((1, 1), (report.swept, report.settlements.applied));
        assert!(world.row(pending).is_none());
        assert!(world.row(batched).unwrap().batch.is_some());
        assert_eq!(vec!["Ancient vase".to_owned()], world.state().generated);
    }

    #[tokio::test]
    async fn should_leave_work_for_the_batches_while_the_provider_is_unavailable() {
        let world = World::default();
        let work = world.add_pending("Ancient vase");
        world.state().generation_error = Some(|| EmbeddingError::DeadlineExceeded {
            deadline: std::time::Duration::from_secs(1),
        });

        let report = handler(&world).execute(&system()).await.unwrap();

        assert_eq!(1, report.settlements.requeued);
        let row = world.row(work).unwrap();
        assert_eq!((0, false), (row.pending.attempts, row.parked));
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
        let pending = handler.reader.pending(10).await.unwrap().remove(0);
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
