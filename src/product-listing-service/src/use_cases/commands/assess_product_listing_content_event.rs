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
        if source.current_event_id != command.event_id {
            return Ok(result(AssessProductListingContentEventOutcome::Stale));
        }
        if source.event_group != DOMAIN_EVENT_GROUP {
            return Ok(result(
                AssessProductListingContentEventOutcome::IgnoredEvent,
            ));
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
