//! Schedules the embedding of a listing's embedding revision. Embedding itself runs in provider
//! batches ([`super::embed_product_listing_batches`]), and whatever is still pending before the
//! daily match in the [sweep](super::sweep_product_listing_embeddings); this use case only
//! records the revision as pending work.

use crate::ports::{
    ProductEmbeddingScheduleOutcome, ProductEmbeddingWorkRef, ProductListingEmbeddingQueue,
    ProductListingEmbeddingQueueError, ProductListingEmbeddingQueueFactory,
    ProductListingEmbeddingSourceEvent, ProductListingEmbeddingSourceReadError,
    ProductListingEmbeddingSourceReader,
};
use application::error::{BoxError, box_error};
use application::operation_context::{OperationAuthorizationError, OperationContext};
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use product_listing_core::product_listing_id::ProductListingId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbedProductListingCommand {
    pub event_id: EventId,
    pub product_listing_id: ProductListingId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedProductListingEventOutcome {
    /// The revision waits for the next embedding batch.
    Scheduled,
    /// The embedding of the revision is already committed.
    Duplicate,
    Stale,
    ProductListingNotFound,
    IgnoredEvent,
    MissingTitle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbedProductListingEventResult {
    pub outcome: EmbedProductListingEventOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum EmbedProductListingEventError {
    #[error("service or system principal required for product embedding")]
    ServiceOrSystemPrincipalRequired,
    #[error("failed to read product embedding source")]
    SourceReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin product embedding transaction")]
    BeginTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to schedule product embedding")]
    ScheduleFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to commit product embedding transaction")]
    CommitTransactionFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait EmbedProductListingEventUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: EmbedProductListingCommand,
    ) -> Result<EmbedProductListingEventResult, EmbedProductListingEventError>;
}

pub struct EmbedProductListingEventHandler<S, U, Q> {
    sources: S,
    unit_of_work: U,
    queue: Q,
}

impl<S, U, Q> EmbedProductListingEventHandler<S, U, Q> {
    pub fn new(sources: S, unit_of_work: U, queue: Q) -> Self {
        Self {
            sources,
            unit_of_work,
            queue,
        }
    }
}

#[async_trait::async_trait]
impl<S, U, Q> EmbedProductListingEventUseCase for EmbedProductListingEventHandler<S, U, Q>
where
    S: ProductListingEmbeddingSourceReader,
    U: UnitOfWork,
    Q: ProductListingEmbeddingQueueFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "embed_product_event",
        skip_all,
        fields(
            product_listing_id = %command.product_listing_id,
            event_id = %command.event_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: EmbedProductListingCommand,
    ) -> Result<EmbedProductListingEventResult, EmbedProductListingEventError> {
        context
            .require()
            .service_or_system()
            .authorize::<EmbedProductListingEventError>()?;
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }

        let Some(source) = self
            .sources
            .find_source(command.event_id, command.product_listing_id)
            .await?
        else {
            return Ok(result(
                EmbedProductListingEventOutcome::ProductListingNotFound,
            ));
        };
        if source.embedding_source_event_id != command.event_id {
            return Ok(result(EmbedProductListingEventOutcome::Stale));
        }
        if !matches!(
            source.event,
            ProductListingEmbeddingSourceEvent::Discovered
                | ProductListingEmbeddingSourceEvent::ChangedContent
        ) {
            return Ok(result(EmbedProductListingEventOutcome::IgnoredEvent));
        }
        if source.title.is_none() {
            return Ok(result(EmbedProductListingEventOutcome::MissingTitle));
        }

        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            EmbedProductListingEventError::BeginTransactionFailed {
                source: box_error(source),
            }
        })?;
        let outcome = self
            .queue
            .in_transaction(&mut tx)
            .schedule(ProductEmbeddingWorkRef {
                product_listing_id: command.product_listing_id,
                source_event_id: command.event_id,
            })
            .await?;
        tx.commit().await.map_err(|source| {
            EmbedProductListingEventError::CommitTransactionFailed {
                source: box_error(source),
            }
        })?;

        Ok(result(match outcome {
            ProductEmbeddingScheduleOutcome::Scheduled => {
                EmbedProductListingEventOutcome::Scheduled
            }
            ProductEmbeddingScheduleOutcome::AlreadyEmbedded => {
                EmbedProductListingEventOutcome::Duplicate
            }
            ProductEmbeddingScheduleOutcome::Superseded => EmbedProductListingEventOutcome::Stale,
        }))
    }
}

fn result(outcome: EmbedProductListingEventOutcome) -> EmbedProductListingEventResult {
    EmbedProductListingEventResult { outcome }
}

impl From<OperationAuthorizationError> for EmbedProductListingEventError {
    fn from(_: OperationAuthorizationError) -> Self {
        Self::ServiceOrSystemPrincipalRequired
    }
}

impl From<ProductListingEmbeddingSourceReadError> for EmbedProductListingEventError {
    fn from(source: ProductListingEmbeddingSourceReadError) -> Self {
        Self::SourceReadFailed {
            source: box_error(source),
        }
    }
}

impl From<ProductListingEmbeddingQueueError> for EmbedProductListingEventError {
    fn from(source: ProductListingEmbeddingQueueError) -> Self {
        Self::ScheduleFailed {
            source: box_error(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        PendingProductEmbedding, ProductEmbeddingAssignment, ProductEmbeddingBatchKey,
        ProductEmbeddingClaim, ProductListingEmbeddingSource,
    };
    use application::operation_context::{CorrelationId, Principal, RequestId};
    use application::transaction::TransactionError;
    use embedding::EmbeddingBatchId;
    use localization::{Language, Localized};
    use product_listing_core::{description::Description, title::Title};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use url::Url;

    struct State {
        source: Option<ProductListingEmbeddingSource>,
        schedule_outcome: ProductEmbeddingScheduleOutcome,
        scheduled: Vec<ProductEmbeddingWorkRef>,
        begins: usize,
        commits: usize,
    }
    type SharedState = Arc<Mutex<State>>;
    struct Sources(SharedState);
    struct UnitOfWorkFake(SharedState);
    struct QueueFactory(SharedState);
    struct Tx(SharedState);
    struct Queue(SharedState);

    fn lock(state: &SharedState) -> std::sync::MutexGuard<'_, State> {
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn state() -> SharedState {
        let event_id = EventId::new();
        Arc::new(Mutex::new(State {
            source: Some(ProductListingEmbeddingSource {
                product_listing_id: ProductListingId::new(),
                event_id,
                embedding_source_event_id: event_id,
                event: ProductListingEmbeddingSourceEvent::Discovered,
                title: Some(Localized::new(Language::De, Title::from("Ancient vase"))),
                description: Some(Localized::new(
                    Language::De,
                    Description::from("Painted clay"),
                )),
                image_url: Some(Url::parse("https://example.test/vase.jpg").unwrap()),
            }),
            schedule_outcome: ProductEmbeddingScheduleOutcome::Scheduled,
            scheduled: Vec::new(),
            begins: 0,
            commits: 0,
        }))
    }

    fn command(state: &SharedState) -> EmbedProductListingCommand {
        let source = lock(state).source.clone().unwrap();
        EmbedProductListingCommand {
            event_id: source.event_id,
            product_listing_id: source.product_listing_id,
        }
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn handler(
        state: &SharedState,
    ) -> EmbedProductListingEventHandler<Sources, UnitOfWorkFake, QueueFactory> {
        EmbedProductListingEventHandler::new(
            Sources(Arc::clone(state)),
            UnitOfWorkFake(Arc::clone(state)),
            QueueFactory(Arc::clone(state)),
        )
    }

    async fn execute(
        state: &SharedState,
    ) -> Result<EmbedProductListingEventOutcome, EmbedProductListingEventError> {
        handler(state)
            .execute(&context(Principal::System), command(state))
            .await
            .map(|result| result.outcome)
    }

    #[async_trait::async_trait]
    impl ProductListingEmbeddingSourceReader for Sources {
        async fn find_source(
            &self,
            _: EventId,
            _: ProductListingId,
        ) -> Result<Option<ProductListingEmbeddingSource>, ProductListingEmbeddingSourceReadError>
        {
            Ok(lock(&self.0).source.clone())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for UnitOfWorkFake {
        type Tx = Tx;
        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            lock(&self.0).begins += 1;
            Ok(Tx(Arc::clone(&self.0)))
        }
    }

    #[async_trait::async_trait]
    impl Transaction for Tx {
        async fn commit(self) -> Result<(), TransactionError> {
            lock(&self.0).commits += 1;
            Ok(())
        }
    }

    impl ProductListingEmbeddingQueueFactory<Tx> for QueueFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut Tx,
        ) -> impl ProductListingEmbeddingQueue + 'tx {
            Queue(Arc::clone(&self.0))
        }
    }

    type QueueResult<T> = Result<T, ProductListingEmbeddingQueueError>;

    #[async_trait::async_trait]
    impl ProductListingEmbeddingQueue for Queue {
        async fn schedule(
            &mut self,
            work: ProductEmbeddingWorkRef,
        ) -> QueueResult<ProductEmbeddingScheduleOutcome> {
            let mut state = lock(&self.0);
            state.scheduled.push(work);
            Ok(state.schedule_outcome)
        }
        async fn discard_superseded(&mut self) -> QueueResult<u64> {
            unreachable!()
        }
        async fn lock_pending(&mut self, _: u32) -> QueueResult<Vec<PendingProductEmbedding>> {
            unreachable!()
        }
        async fn create_batch(
            &mut self,
            _: ProductEmbeddingBatchKey,
            _: &[ProductEmbeddingAssignment],
        ) -> QueueResult<()> {
            unreachable!()
        }
        async fn record_submission(
            &mut self,
            _: ProductEmbeddingBatchKey,
            _: &EmbeddingBatchId,
        ) -> QueueResult<()> {
            unreachable!()
        }
        async fn close_batch(&mut self, _: ProductEmbeddingBatchKey) -> QueueResult<()> {
            unreachable!()
        }
        async fn close_abandoned_submissions(&mut self, _: Duration) -> QueueResult<u64> {
            unreachable!()
        }
        async fn complete(
            &mut self,
            _: ProductEmbeddingWorkRef,
            _: ProductEmbeddingClaim,
        ) -> QueueResult<()> {
            unreachable!()
        }
        async fn requeue(
            &mut self,
            _: ProductEmbeddingWorkRef,
            _: ProductEmbeddingClaim,
            _: u16,
        ) -> QueueResult<()> {
            unreachable!()
        }
        async fn park(
            &mut self,
            _: ProductEmbeddingWorkRef,
            _: ProductEmbeddingClaim,
            _: u16,
        ) -> QueueResult<()> {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn should_schedule_the_current_revision_of_discovered_or_changed_content() {
        for event in [
            ProductListingEmbeddingSourceEvent::Discovered,
            ProductListingEmbeddingSourceEvent::ChangedContent,
        ] {
            let state = state();
            lock(&state).source.as_mut().unwrap().event = event;

            assert_eq!(
                EmbedProductListingEventOutcome::Scheduled,
                execute(&state).await.unwrap()
            );

            let command = command(&state);
            let state = lock(&state);
            assert_eq!(
                vec![ProductEmbeddingWorkRef {
                    product_listing_id: command.product_listing_id,
                    source_event_id: command.event_id,
                }],
                state.scheduled
            );
            assert_eq!((1, 1), (state.begins, state.commits));
        }
    }

    #[tokio::test]
    async fn should_report_an_embedded_or_superseded_revision_as_settled() {
        for (schedule_outcome, expected) in [
            (
                ProductEmbeddingScheduleOutcome::AlreadyEmbedded,
                EmbedProductListingEventOutcome::Duplicate,
            ),
            (
                ProductEmbeddingScheduleOutcome::Superseded,
                EmbedProductListingEventOutcome::Stale,
            ),
        ] {
            let state = state();
            lock(&state).schedule_outcome = schedule_outcome;

            assert_eq!(expected, execute(&state).await.unwrap());
        }
    }

    #[tokio::test]
    async fn should_skip_stale_ignored_and_untitled_revisions_without_a_transaction() {
        let cases: [(fn(&mut ProductListingEmbeddingSource), _); 3] = [
            (
                |source| source.embedding_source_event_id = EventId::new(),
                EmbedProductListingEventOutcome::Stale,
            ),
            (
                |source| source.event = ProductListingEmbeddingSourceEvent::Other,
                EmbedProductListingEventOutcome::IgnoredEvent,
            ),
            (
                |source| source.title = None,
                EmbedProductListingEventOutcome::MissingTitle,
            ),
        ];
        for (change, expected) in cases {
            let state = state();
            change(lock(&state).source.as_mut().unwrap());

            assert_eq!(expected, execute(&state).await.unwrap());
            assert_eq!(0, lock(&state).begins);
        }
    }

    #[tokio::test]
    async fn should_reject_user_principal_before_source_read() {
        let state = state();
        let result = handler(&state)
            .execute(
                &context(Principal::User(user_core::user_id::UserId::new())),
                command(&state),
            )
            .await;
        assert!(matches!(
            result,
            Err(EmbedProductListingEventError::ServiceOrSystemPrincipalRequired)
        ));
        assert!(lock(&state).scheduled.is_empty());
    }
}
