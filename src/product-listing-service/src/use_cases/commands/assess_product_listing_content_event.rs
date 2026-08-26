use crate::ports::{
    ProductListingContentAssessmentSourceReadError, ProductListingContentAssessmentSourceReader,
    ProductListingContentAssessmentWrite, ProductListingContentAssessmentWriteError,
    ProductListingContentAssessmentWriteOutcome, ProductListingContentAssessmentWriter,
    ProductListingContentAssessmentWriterFactory,
};
use application::error::{BoxError, box_error};
use application::operation_context::{OperationAuthorizationError, OperationContext};
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use product_listing_core::{
    content_policy::assess_listing_text, product_listing_id::ProductListingId,
};

const DOMAIN_EVENT_GROUP: &str = "DOMAIN";
const CONTENT_SOURCE_EVENT_TYPE: &str = "PRODUCT_LISTING_CREATED";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssessProductListingContentCommand {
    pub event_id: EventId,
    pub product_listing_id: ProductListingId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssessProductListingContentEventOutcome {
    Applied,
    Duplicate,
    Stale,
    ProductListingNotFound,
    IgnoredEvent,
    Cleared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssessProductListingContentEventResult {
    pub outcome: AssessProductListingContentEventOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum AssessProductListingContentEventError {
    #[error("service or system principal required for product content assessment")]
    ServiceOrSystemPrincipalRequired,
    #[error("failed to read product content assessment source")]
    SourceReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin product content assessment transaction")]
    BeginTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to persist product content assessment")]
    AssessmentWriteFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to commit product content assessment transaction")]
    CommitTransactionFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait AssessProductListingContentEventUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: AssessProductListingContentCommand,
    ) -> Result<AssessProductListingContentEventResult, AssessProductListingContentEventError>;
}

pub struct AssessProductListingContentEventHandler<S, U, W> {
    sources: S,
    unit_of_work: U,
    assessments: W,
}

impl<S, U, W> AssessProductListingContentEventHandler<S, U, W> {
    pub fn new(sources: S, unit_of_work: U, assessments: W) -> Self {
        Self {
            sources,
            unit_of_work,
            assessments,
        }
    }
}

#[async_trait::async_trait]
impl<S, U, W> AssessProductListingContentEventUseCase
    for AssessProductListingContentEventHandler<S, U, W>
where
    S: ProductListingContentAssessmentSourceReader,
    U: UnitOfWork,
    W: ProductListingContentAssessmentWriterFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "assess_product_content_event",
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
        command: AssessProductListingContentCommand,
    ) -> Result<AssessProductListingContentEventResult, AssessProductListingContentEventError> {
        context
            .require()
            .service_or_system()
            .authorize::<AssessProductListingContentEventError>()?;
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }

        let Some(source) = self
            .sources
            .find_source(command.event_id, command.product_listing_id)
            .await
            .map_err(
                |source| AssessProductListingContentEventError::SourceReadFailed {
                    source: box_error(source),
                },
            )?
        else {
            return Ok(result(
                AssessProductListingContentEventOutcome::ProductListingNotFound,
            ));
        };
        if source.event_group != DOMAIN_EVENT_GROUP
            || source.event_type != CONTENT_SOURCE_EVENT_TYPE
        {
            return Ok(result(
                AssessProductListingContentEventOutcome::IgnoredEvent,
            ));
        }
        if source.current_content_source_event_id != command.event_id {
            return Ok(result(AssessProductListingContentEventOutcome::Stale));
        }
        let decision = assess_listing_text(
            source.title.as_deref().map(AsRef::as_ref),
            source.description.as_deref().map(AsRef::as_ref),
        );

        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            AssessProductListingContentEventError::BeginTransactionFailed {
                source: box_error(source),
            }
        })?;
        let outcome = self
            .assessments
            .in_transaction(&mut tx)
            .apply(&ProductListingContentAssessmentWrite {
                product_listing_id: command.product_listing_id,
                source_event_id: command.event_id,
                decision,
            })
            .await
            .map_err(
                |source| AssessProductListingContentEventError::AssessmentWriteFailed {
                    source: box_error(source),
                },
            )?;
        tx.commit().await.map_err(|source| {
            AssessProductListingContentEventError::CommitTransactionFailed {
                source: box_error(source),
            }
        })?;

        let outcome = match outcome {
            ProductListingContentAssessmentWriteOutcome::Applied => {
                AssessProductListingContentEventOutcome::Applied
            }
            ProductListingContentAssessmentWriteOutcome::Cleared => {
                AssessProductListingContentEventOutcome::Cleared
            }
            ProductListingContentAssessmentWriteOutcome::Duplicate => {
                AssessProductListingContentEventOutcome::Duplicate
            }
            ProductListingContentAssessmentWriteOutcome::Stale => {
                AssessProductListingContentEventOutcome::Stale
            }
            ProductListingContentAssessmentWriteOutcome::ProductListingNotFound => {
                AssessProductListingContentEventOutcome::ProductListingNotFound
            }
        };
        if matches!(
            outcome,
            AssessProductListingContentEventOutcome::Applied
                | AssessProductListingContentEventOutcome::Cleared
        ) {
            tracing::info!(event = "product.content_assessed", actor_type = context.principal.kind(), product_listing_id = %command.product_listing_id, source_event_id = %command.event_id, outcome = ?outcome);
        }
        Ok(result(outcome))
    }
}

fn result(
    outcome: AssessProductListingContentEventOutcome,
) -> AssessProductListingContentEventResult {
    AssessProductListingContentEventResult { outcome }
}

impl From<OperationAuthorizationError> for AssessProductListingContentEventError {
    fn from(_: OperationAuthorizationError) -> Self {
        Self::ServiceOrSystemPrincipalRequired
    }
}

impl From<ProductListingContentAssessmentSourceReadError>
    for AssessProductListingContentEventError
{
    fn from(source: ProductListingContentAssessmentSourceReadError) -> Self {
        Self::SourceReadFailed {
            source: box_error(source),
        }
    }
}

impl From<ProductListingContentAssessmentWriteError> for AssessProductListingContentEventError {
    fn from(source: ProductListingContentAssessmentWriteError) -> Self {
        Self::AssessmentWriteFailed {
            source: box_error(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::ProductListingContentAssessmentSource;
    use application::{
        operation_context::{CorrelationId, Principal, RequestId},
        transaction::TransactionError,
    };
    use product_listing_core::{description::Description, title::Title};
    use std::sync::{Arc, Mutex};

    struct State {
        source: ProductListingContentAssessmentSource,
        writes: Vec<ProductListingContentAssessmentWrite>,
        begins: usize,
        commits: usize,
    }

    type SharedState = Arc<Mutex<State>>;

    struct Sources(SharedState);
    struct UnitOfWorkFake(SharedState);
    struct Tx(SharedState);
    struct WriterFactory(SharedState);
    struct Writer(SharedState);

    fn lock(state: &SharedState) -> std::sync::MutexGuard<'_, State> {
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn state() -> SharedState {
        let product_listing_id = ProductListingId::new();
        let content_source_event_id = EventId::new();
        Arc::new(Mutex::new(State {
            source: ProductListingContentAssessmentSource {
                product_listing_id,
                event_id: content_source_event_id,
                current_content_source_event_id: content_source_event_id,
                event_group: DOMAIN_EVENT_GROUP.to_owned(),
                event_type: "PRODUCT_LISTING_CREATED".to_owned(),
                title: Some(Title::from("Ancient vase")),
                description: Some(Description::from("Painted clay")),
            },
            writes: Vec::new(),
            begins: 0,
            commits: 0,
        }))
    }

    fn command(state: &SharedState) -> AssessProductListingContentCommand {
        let source = &lock(state).source;
        AssessProductListingContentCommand {
            event_id: source.event_id,
            product_listing_id: source.product_listing_id,
        }
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn handler(
        state: &SharedState,
    ) -> AssessProductListingContentEventHandler<Sources, UnitOfWorkFake, WriterFactory> {
        AssessProductListingContentEventHandler::new(
            Sources(Arc::clone(state)),
            UnitOfWorkFake(Arc::clone(state)),
            WriterFactory(Arc::clone(state)),
        )
    }

    #[async_trait::async_trait]
    impl ProductListingContentAssessmentSourceReader for Sources {
        async fn find_source(
            &self,
            _: EventId,
            _: ProductListingId,
        ) -> Result<
            Option<ProductListingContentAssessmentSource>,
            ProductListingContentAssessmentSourceReadError,
        > {
            Ok(Some(lock(&self.0).source.clone()))
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for UnitOfWorkFake {
        type Tx = Tx;

        async fn begin(&self) -> Result<Self::Tx, application::transaction::TransactionError> {
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

    impl ProductListingContentAssessmentWriterFactory<Tx> for WriterFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut Tx,
        ) -> impl ProductListingContentAssessmentWriter + 'tx {
            Writer(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl ProductListingContentAssessmentWriter for Writer {
        async fn apply(
            &mut self,
            write: &ProductListingContentAssessmentWrite,
        ) -> Result<
            ProductListingContentAssessmentWriteOutcome,
            ProductListingContentAssessmentWriteError,
        > {
            lock(&self.0).writes.push(*write);
            Ok(ProductListingContentAssessmentWriteOutcome::Applied)
        }
    }

    #[tokio::test]
    async fn should_ignore_non_content_source_domain_event_without_writing() {
        let state = state();
        let command = command(&state);
        lock(&state).source.event_type = "PRODUCT_LISTING_PRICE_CHANGED".to_owned();

        let result = handler(&state).execute(&context(), command).await;

        assert!(matches!(
            result,
            Ok(AssessProductListingContentEventResult {
                outcome: AssessProductListingContentEventOutcome::IgnoredEvent
            })
        ));
        let state = lock(&state);
        assert_eq!(0, state.begins);
        assert_eq!(0, state.commits);
        assert!(state.writes.is_empty());
    }

    #[tokio::test]
    async fn should_assess_current_content_after_generic_price_or_enrichment_revision() {
        let state = state();
        let result = handler(&state).execute(&context(), command(&state)).await;

        assert!(matches!(
            result,
            Ok(AssessProductListingContentEventResult {
                outcome: AssessProductListingContentEventOutcome::Applied
            })
        ));
        let state = lock(&state);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.commits);
        assert!(matches!(
            state.writes.as_slice(),
            [ProductListingContentAssessmentWrite { source_event_id, .. }]
                if *source_event_id == state.source.current_content_source_event_id
        ));
    }
}
