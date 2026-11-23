//! Schedules title translation for a listing's content revision. Translation itself runs in
//! provider batches ([`super::translate_product_listing_title_batches`]); this use case only
//! records the revision as pending work.

use crate::ports::{
    ProductListingTitleTranslationQueue, ProductListingTitleTranslationQueueError,
    ProductListingTitleTranslationQueueFactory, ProductListingTranslationSourceEvent,
    ProductListingTranslationSourceReadError, ProductListingTranslationSourceReader,
    TitleTranslationScheduleOutcome, TitleTranslationWorkRef,
};
use application::error::{BoxError, box_error};
use application::operation_context::{OperationAuthorizationError, OperationContext};
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use product_listing_core::product_listing_id::ProductListingId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranslateProductListingCommand {
    pub event_id: EventId,
    pub product_listing_id: ProductListingId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranslateProductListingEventOutcome {
    /// The revision waits for the next translation batch.
    Scheduled,
    /// Translations of the revision are already committed.
    Duplicate,
    Stale,
    ProductListingNotFound,
    IgnoredEvent,
    MissingTitle,
    MissingTitleLanguage,
    EmptyTitle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranslateProductListingEventResult {
    pub outcome: TranslateProductListingEventOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum TranslateProductListingEventError {
    #[error("service or system principal required for product translation")]
    ServiceOrSystemPrincipalRequired,
    #[error("failed to read product translation source")]
    SourceReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin product translation transaction")]
    BeginTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to schedule product title translation")]
    ScheduleFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to commit product translation transaction")]
    CommitTransactionFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait TranslateProductListingEventUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: TranslateProductListingCommand,
    ) -> Result<TranslateProductListingEventResult, TranslateProductListingEventError>;
}

pub struct TranslateProductListingEventHandler<S, U, Q> {
    sources: S,
    unit_of_work: U,
    queue: Q,
}

impl<S, U, Q> TranslateProductListingEventHandler<S, U, Q> {
    pub fn new(sources: S, unit_of_work: U, queue: Q) -> Self {
        Self {
            sources,
            unit_of_work,
            queue,
        }
    }
}

#[async_trait::async_trait]
impl<S, U, Q> TranslateProductListingEventUseCase for TranslateProductListingEventHandler<S, U, Q>
where
    S: ProductListingTranslationSourceReader,
    U: UnitOfWork,
    Q: ProductListingTitleTranslationQueueFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "translate_product_event",
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
        command: TranslateProductListingCommand,
    ) -> Result<TranslateProductListingEventResult, TranslateProductListingEventError> {
        context
            .require()
            .service_or_system()
            .authorize::<TranslateProductListingEventError>()?;
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }

        let Some(source) = self
            .sources
            .find_source(command.event_id, command.product_listing_id)
            .await?
        else {
            return Ok(result(
                TranslateProductListingEventOutcome::ProductListingNotFound,
            ));
        };

        if source.content_source_event_id != command.event_id {
            return Ok(result(TranslateProductListingEventOutcome::Stale));
        }
        if !matches!(
            source.event,
            ProductListingTranslationSourceEvent::Discovered
                | ProductListingTranslationSourceEvent::ChangedText
        ) {
            return Ok(result(TranslateProductListingEventOutcome::IgnoredEvent));
        }
        let Some(title) = source.title else {
            return Ok(result(TranslateProductListingEventOutcome::MissingTitle));
        };
        if source.title_language.is_none() {
            return Ok(result(
                TranslateProductListingEventOutcome::MissingTitleLanguage,
            ));
        }
        if title.as_ref().is_empty() {
            return Ok(result(TranslateProductListingEventOutcome::EmptyTitle));
        }

        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            TranslateProductListingEventError::BeginTransactionFailed {
                source: box_error(source),
            }
        })?;
        let outcome = self
            .queue
            .in_transaction(&mut tx)
            .schedule(TitleTranslationWorkRef {
                product_listing_id: command.product_listing_id,
                source_event_id: command.event_id,
            })
            .await?;
        tx.commit().await.map_err(|source| {
            TranslateProductListingEventError::CommitTransactionFailed {
                source: box_error(source),
            }
        })?;

        Ok(result(match outcome {
            TitleTranslationScheduleOutcome::Scheduled => {
                TranslateProductListingEventOutcome::Scheduled
            }
            TitleTranslationScheduleOutcome::AlreadyTranslated => {
                TranslateProductListingEventOutcome::Duplicate
            }
            TitleTranslationScheduleOutcome::Superseded => {
                TranslateProductListingEventOutcome::Stale
            }
        }))
    }
}

fn result(outcome: TranslateProductListingEventOutcome) -> TranslateProductListingEventResult {
    TranslateProductListingEventResult { outcome }
}

impl From<OperationAuthorizationError> for TranslateProductListingEventError {
    fn from(_error: OperationAuthorizationError) -> Self {
        Self::ServiceOrSystemPrincipalRequired
    }
}

impl From<ProductListingTranslationSourceReadError> for TranslateProductListingEventError {
    fn from(source: ProductListingTranslationSourceReadError) -> Self {
        Self::SourceReadFailed {
            source: box_error(source),
        }
    }
}

impl From<ProductListingTitleTranslationQueueError> for TranslateProductListingEventError {
    fn from(source: ProductListingTitleTranslationQueueError) -> Self {
        Self::ScheduleFailed {
            source: box_error(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        PendingTitleTranslation, ProductListingTranslationSource, TitleTranslationAssignment,
        TitleTranslationBatchId, TitleTranslationBatchKey, TitleTranslationClaim,
        TitleTranslationMode,
    };
    use application::operation_context::{CorrelationId, Principal, RequestId};
    use application::transaction::TransactionError;
    use localization::Language;
    use product_listing_core::title::Title;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    struct State {
        source: Option<ProductListingTranslationSource>,
        schedule_outcome: TitleTranslationScheduleOutcome,
        scheduled: Vec<TitleTranslationWorkRef>,
        commits: usize,
    }

    type SharedState = Arc<Mutex<State>>;

    #[derive(Clone)]
    struct Sources(SharedState);
    #[derive(Clone)]
    struct UnitOfWorkFake(SharedState);
    #[derive(Clone)]
    struct QueueFactory(SharedState);
    struct Tx(SharedState);
    struct Queue(SharedState);

    fn lock(state: &SharedState) -> std::sync::MutexGuard<'_, State> {
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[async_trait::async_trait]
    impl ProductListingTranslationSourceReader for Sources {
        async fn find_source(
            &self,
            _event_id: EventId,
            _product_listing_id: ProductListingId,
        ) -> Result<Option<ProductListingTranslationSource>, ProductListingTranslationSourceReadError>
        {
            Ok(lock(&self.0).source.clone())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for UnitOfWorkFake {
        type Tx = Tx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
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

    impl ProductListingTitleTranslationQueueFactory<Tx> for QueueFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut Tx,
        ) -> impl ProductListingTitleTranslationQueue + 'tx {
            Queue(Arc::clone(&self.0))
        }
    }

    type QueueResult<T> = Result<T, ProductListingTitleTranslationQueueError>;

    #[async_trait::async_trait]
    impl ProductListingTitleTranslationQueue for Queue {
        async fn schedule(
            &mut self,
            work: TitleTranslationWorkRef,
        ) -> QueueResult<TitleTranslationScheduleOutcome> {
            let mut state = lock(&self.0);
            state.scheduled.push(work);
            Ok(state.schedule_outcome)
        }

        async fn discard_superseded(&mut self) -> QueueResult<u64> {
            unreachable!()
        }

        async fn lock_pending(&mut self, _: u32) -> QueueResult<Vec<PendingTitleTranslation>> {
            unreachable!()
        }

        async fn create_batch(
            &mut self,
            _: TitleTranslationBatchKey,
            _: &[TitleTranslationAssignment],
        ) -> QueueResult<()> {
            unreachable!()
        }

        async fn record_submission(
            &mut self,
            _: TitleTranslationBatchKey,
            _: &TitleTranslationBatchId,
        ) -> QueueResult<()> {
            unreachable!()
        }

        async fn close_batch(&mut self, _: TitleTranslationBatchKey) -> QueueResult<()> {
            unreachable!()
        }

        async fn close_abandoned_submissions(&mut self, _: Duration) -> QueueResult<u64> {
            unreachable!()
        }

        async fn complete(
            &mut self,
            _: TitleTranslationWorkRef,
            _: TitleTranslationClaim,
        ) -> QueueResult<()> {
            unreachable!()
        }

        async fn requeue(
            &mut self,
            _: TitleTranslationWorkRef,
            _: TitleTranslationClaim,
            _: TitleTranslationMode,
            _: u16,
        ) -> QueueResult<()> {
            unreachable!()
        }

        async fn park(
            &mut self,
            _: TitleTranslationWorkRef,
            _: TitleTranslationClaim,
            _: u16,
        ) -> QueueResult<()> {
            unreachable!()
        }
    }

    fn state_with_source() -> SharedState {
        let event_id = EventId::new();
        Arc::new(Mutex::new(State {
            source: Some(ProductListingTranslationSource {
                product_listing_id: ProductListingId::new(),
                event_id,
                content_source_event_id: event_id,
                event: ProductListingTranslationSourceEvent::Discovered,
                title: Some(Title::from("Ancient vase")),
                title_language: Some(Language::De),
            }),
            schedule_outcome: TitleTranslationScheduleOutcome::Scheduled,
            scheduled: Vec::new(),
            commits: 0,
        }))
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn command(state: &SharedState) -> TranslateProductListingCommand {
        lock(state).source.as_ref().map_or(
            TranslateProductListingCommand {
                event_id: EventId::new(),
                product_listing_id: ProductListingId::new(),
            },
            |source| TranslateProductListingCommand {
                event_id: source.event_id,
                product_listing_id: source.product_listing_id,
            },
        )
    }

    async fn execute(
        state: &SharedState,
        principal: Principal,
    ) -> Result<TranslateProductListingEventOutcome, TranslateProductListingEventError> {
        TranslateProductListingEventHandler::new(
            Sources(Arc::clone(state)),
            UnitOfWorkFake(Arc::clone(state)),
            QueueFactory(Arc::clone(state)),
        )
        .execute(&context(principal), command(state))
        .await
        .map(|result| result.outcome)
    }

    #[tokio::test]
    async fn should_schedule_the_current_text_revision_of_a_titled_listing() {
        for event in [
            ProductListingTranslationSourceEvent::Discovered,
            ProductListingTranslationSourceEvent::ChangedText,
        ] {
            let state = state_with_source();
            lock(&state).source.as_mut().unwrap().event = event;
            let command = command(&state);

            let outcome = execute(&state, Principal::System).await.unwrap();

            assert_eq!(TranslateProductListingEventOutcome::Scheduled, outcome);
            let state = lock(&state);
            assert_eq!(
                vec![TitleTranslationWorkRef {
                    product_listing_id: command.product_listing_id,
                    source_event_id: command.event_id,
                }],
                state.scheduled
            );
            assert_eq!(1, state.commits);
        }
    }

    #[tokio::test]
    async fn should_report_what_the_guarded_schedule_found() {
        for (found, expected) in [
            (
                TitleTranslationScheduleOutcome::AlreadyTranslated,
                TranslateProductListingEventOutcome::Duplicate,
            ),
            (
                TitleTranslationScheduleOutcome::Superseded,
                TranslateProductListingEventOutcome::Stale,
            ),
        ] {
            let state = state_with_source();
            lock(&state).schedule_outcome = found;

            assert_eq!(expected, execute(&state, Principal::System).await.unwrap());
        }
    }

    #[tokio::test]
    async fn should_schedule_nothing_for_revisions_without_a_translatable_title() {
        type Change = fn(&mut ProductListingTranslationSource);
        let cases: [(Change, TranslateProductListingEventOutcome); 5] = [
            (
                |source| source.content_source_event_id = EventId::new(),
                TranslateProductListingEventOutcome::Stale,
            ),
            (
                |source| source.event = ProductListingTranslationSourceEvent::Other,
                TranslateProductListingEventOutcome::IgnoredEvent,
            ),
            (
                |source| source.title = None,
                TranslateProductListingEventOutcome::MissingTitle,
            ),
            (
                |source| source.title_language = None,
                TranslateProductListingEventOutcome::MissingTitleLanguage,
            ),
            (
                |source| source.title = Some(Title::from("")),
                TranslateProductListingEventOutcome::EmptyTitle,
            ),
        ];
        for (change, expected) in cases {
            let state = state_with_source();
            change(lock(&state).source.as_mut().unwrap());

            assert_eq!(expected, execute(&state, Principal::System).await.unwrap());
            assert!(lock(&state).scheduled.is_empty());
            assert_eq!(0, lock(&state).commits);
        }

        let state = state_with_source();
        lock(&state).source = None;
        assert_eq!(
            TranslateProductListingEventOutcome::ProductListingNotFound,
            execute(&state, Principal::System).await.unwrap()
        );
    }

    #[tokio::test]
    async fn should_reject_user_principal() {
        let state = state_with_source();

        let result = execute(&state, Principal::User(user_core::user_id::UserId::new())).await;

        assert!(matches!(
            result,
            Err(TranslateProductListingEventError::ServiceOrSystemPrincipalRequired)
        ));
        assert!(lock(&state).scheduled.is_empty());
    }
}
