use crate::ports::{
    ProductListingEmbeddingSourceReadError, ProductListingEmbeddingSourceReader,
    ProductListingEmbeddingWrite, ProductListingEmbeddingWriteError,
    ProductListingEmbeddingWriteOutcome, ProductListingEmbeddingWriter,
    ProductListingEmbeddingWriterFactory,
};
use application::error::{BoxError, box_error};
use application::operation_context::{OperationAuthorizationError, OperationContext};
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use embedding::{EmbeddingError, EmbeddingGenerator, EmbeddingImageUrl, EmbeddingText};
use product_listing_core::product_listing_id::ProductListingId;

const CREATED_EVENT_TYPE: &str = "PRODUCT_LISTING_CREATED";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbedProductListingCommand {
    pub event_id: EventId,
    pub product_listing_id: ProductListingId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedProductListingEventOutcome {
    Applied,
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
    #[error("product embedding input is invalid")]
    InvalidInput {
        #[source]
        source: BoxError,
    },
    #[error("product embedding generation failed")]
    GenerationFailed {
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
}

#[async_trait::async_trait]
pub trait EmbedProductListingEventUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: EmbedProductListingCommand,
    ) -> Result<EmbedProductListingEventResult, EmbedProductListingEventError>;
}

pub struct EmbedProductListingEventHandler<S, G, U, W> {
    sources: S,
    generator: G,
    unit_of_work: U,
    embeddings: W,
}

impl<S, G, U, W> EmbedProductListingEventHandler<S, G, U, W> {
    pub fn new(sources: S, generator: G, unit_of_work: U, embeddings: W) -> Self {
        Self {
            sources,
            generator,
            unit_of_work,
            embeddings,
        }
    }
}

#[async_trait::async_trait]
impl<S, G, U, W> EmbedProductListingEventUseCase for EmbedProductListingEventHandler<S, G, U, W>
where
    S: ProductListingEmbeddingSourceReader,
    G: EmbeddingGenerator,
    U: UnitOfWork,
    W: ProductListingEmbeddingWriterFactory<U::Tx>,
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
            .await
            .map_err(|source| EmbedProductListingEventError::SourceReadFailed {
                source: box_error(source),
            })?
        else {
            return Ok(result(
                EmbedProductListingEventOutcome::ProductListingNotFound,
            ));
        };
        if source.current_event_id != command.event_id {
            return Ok(result(EmbedProductListingEventOutcome::Stale));
        }
        if source.event_type != CREATED_EVENT_TYPE {
            return Ok(result(EmbedProductListingEventOutcome::IgnoredEvent));
        }
        let Some(title) = source.title else {
            return Ok(result(EmbedProductListingEventOutcome::MissingTitle));
        };

        let title_text = EmbeddingText::new(title.payload.as_ref()).map_err(|source| {
            EmbedProductListingEventError::InvalidInput {
                source: box_error(source),
            }
        })?;
        let additional_text = source
            .description
            .map(|description| EmbeddingText::new(description.payload.as_ref()))
            .transpose()
            .map_err(|source| EmbedProductListingEventError::InvalidInput {
                source: box_error(source),
            })?;
        let image_url = source
            .image_url
            .map(EmbeddingImageUrl::new)
            .transpose()
            .map_err(|source| EmbedProductListingEventError::InvalidInput {
                source: box_error(source),
            })?;
        let embedding = self
            .generator
            .embed_product(&title_text, additional_text.as_ref(), image_url.as_ref())
            .await
            .map_err(|source| EmbedProductListingEventError::GenerationFailed {
                source: box_error(source),
            })?
            .into_values();

        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            EmbedProductListingEventError::BeginTransactionFailed {
                source: box_error(source),
            }
        })?;
        let outcome = self
            .embeddings
            .in_transaction(&mut tx)
            .apply(&ProductListingEmbeddingWrite {
                product_listing_id: command.product_listing_id,
                source_event_id: command.event_id,
                enrichment_event_id: EventId::new(),
                embedding,
                title,
            })
            .await
            .map_err(
                |source| EmbedProductListingEventError::EmbeddingWriteFailed {
                    source: box_error(source),
                },
            )?;
        tx.commit().await.map_err(|source| {
            EmbedProductListingEventError::CommitTransactionFailed {
                source: box_error(source),
            }
        })?;

        let outcome = match outcome {
            ProductListingEmbeddingWriteOutcome::Applied => {
                EmbedProductListingEventOutcome::Applied
            }
            ProductListingEmbeddingWriteOutcome::Duplicate => {
                EmbedProductListingEventOutcome::Duplicate
            }
            ProductListingEmbeddingWriteOutcome::Stale => EmbedProductListingEventOutcome::Stale,
            ProductListingEmbeddingWriteOutcome::ProductListingNotFound => {
                EmbedProductListingEventOutcome::ProductListingNotFound
            }
        };
        if outcome == EmbedProductListingEventOutcome::Applied {
            tracing::info!(event = "product.embedded", actor_type = context.principal.kind(), product_listing_id = %command.product_listing_id, source_event_id = %command.event_id, outcome = "success");
        }
        Ok(result(outcome))
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
impl From<EmbeddingError> for EmbedProductListingEventError {
    fn from(source: EmbeddingError) -> Self {
        Self::GenerationFailed {
            source: box_error(source),
        }
    }
}
impl From<ProductListingEmbeddingWriteError> for EmbedProductListingEventError {
    fn from(source: ProductListingEmbeddingWriteError) -> Self {
        Self::EmbeddingWriteFailed {
            source: box_error(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::operation_context::{CorrelationId, Principal, RequestId};
    use application::transaction::TransactionError;
    use embedding::{EMBEDDING_DIMENSIONS, EmbeddingVector};
    use localization::{Language, Localized};
    use product_listing_core::{description::Description, title::Title};
    use std::sync::{Arc, Mutex};
    use url::Url;

    struct State {
        source: Option<crate::ports::ProductListingEmbeddingSource>,
        generated_products: Vec<(String, Option<String>, Option<url::Url>)>,
        generation_fails: bool,
        write_outcome: ProductListingEmbeddingWriteOutcome,
        begins: usize,
        commits: usize,
    }
    type SharedState = Arc<Mutex<State>>;
    struct Sources(SharedState);
    struct Generator(SharedState);
    struct UnitOfWorkFake(SharedState);
    struct WriterFactory(SharedState);
    struct Tx(SharedState);
    struct Writer(SharedState);

    fn lock(state: &SharedState) -> std::sync::MutexGuard<'_, State> {
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    fn state() -> SharedState {
        let product_listing_id = ProductListingId::new();
        let event_id = EventId::new();
        Arc::new(Mutex::new(State {
            source: Some(crate::ports::ProductListingEmbeddingSource {
                product_listing_id,
                event_id,
                current_event_id: event_id,
                event_type: "PRODUCT_LISTING_CREATED".to_owned(),
                title: Some(Localized::new(Language::De, Title::from("Ancient vase"))),
                description: Some(Localized::new(
                    Language::De,
                    Description::from("Painted clay"),
                )),
                image_url: Some(
                    Url::parse("https://example.test/vase.jpg")
                        .unwrap_or_else(|error| panic!("test URL invalid: {error}")),
                ),
            }),
            generated_products: Vec::new(),
            generation_fails: false,
            write_outcome: ProductListingEmbeddingWriteOutcome::Applied,
            begins: 0,
            commits: 0,
        }))
    }
    fn command(state: &SharedState) -> EmbedProductListingCommand {
        let source = lock(state)
            .source
            .clone()
            .unwrap_or_else(|| panic!("test source missing"));
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
    ) -> EmbedProductListingEventHandler<Sources, Generator, UnitOfWorkFake, WriterFactory> {
        EmbedProductListingEventHandler::new(
            Sources(Arc::clone(state)),
            Generator(Arc::clone(state)),
            UnitOfWorkFake(Arc::clone(state)),
            WriterFactory(Arc::clone(state)),
        )
    }

    #[async_trait::async_trait]
    impl ProductListingEmbeddingSourceReader for Sources {
        async fn find_source(
            &self,
            _: EventId,
            _: ProductListingId,
        ) -> Result<
            Option<crate::ports::ProductListingEmbeddingSource>,
            ProductListingEmbeddingSourceReadError,
        > {
            Ok(lock(&self.0).source.clone())
        }
    }
    #[async_trait::async_trait]
    impl EmbeddingGenerator for Generator {
        async fn embed_product(
            &self,
            title: &EmbeddingText,
            additional_text: Option<&EmbeddingText>,
            image_url: Option<&EmbeddingImageUrl>,
        ) -> Result<EmbeddingVector, EmbeddingError> {
            let mut state = lock(&self.0);
            state.generated_products.push((
                title.as_str().to_owned(),
                additional_text.map(|text| text.as_str().to_owned()),
                image_url.map(|url| url.as_url().clone()),
            ));
            if state.generation_fails {
                return Err(EmbeddingError::InvalidResponse {
                    reason: "test failure",
                });
            }
            EmbeddingVector::try_new(vec![1.0; EMBEDDING_DIMENSIONS])
        }

        async fn embed_search_query(
            &self,
            _: &EmbeddingText,
        ) -> Result<EmbeddingVector, EmbeddingError> {
            Err(EmbeddingError::InvalidInput {
                reason: "test generator supports products only",
            })
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
    impl ProductListingEmbeddingWriterFactory<Tx> for WriterFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut Tx,
        ) -> impl ProductListingEmbeddingWriter + 'tx {
            Writer(Arc::clone(&self.0))
        }
    }
    #[async_trait::async_trait]
    impl ProductListingEmbeddingWriter for Writer {
        async fn apply(
            &mut self,
            _: &ProductListingEmbeddingWrite,
        ) -> Result<ProductListingEmbeddingWriteOutcome, ProductListingEmbeddingWriteError>
        {
            Ok(lock(&self.0).write_outcome)
        }
    }

    #[tokio::test]
    async fn should_generate_product_listing_created_content_with_first_image_then_commit() {
        let state = state();
        let result = handler(&state)
            .execute(&context(Principal::System), command(&state))
            .await;
        assert!(matches!(
            result,
            Ok(EmbedProductListingEventResult {
                outcome: EmbedProductListingEventOutcome::Applied
            })
        ));
        let state = lock(&state);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.commits);
        assert!(
            matches!(state.generated_products.as_slice(), [(title, Some(additional_text), Some(image_url))] if title == "Ancient vase" && additional_text == "Painted clay" && image_url.as_str() == "https://example.test/vase.jpg")
        );
    }

    #[tokio::test]
    async fn should_ignore_retired_created_event_type_before_generation_or_transaction() {
        let state = state();
        lock(&state)
            .source
            .as_mut()
            .unwrap_or_else(|| panic!("test source missing"))
            .event_type = "DOMAIN_CREATED".to_owned();

        let result = handler(&state)
            .execute(&context(Principal::System), command(&state))
            .await;

        assert!(matches!(
            result,
            Ok(EmbedProductListingEventResult {
                outcome: EmbedProductListingEventOutcome::IgnoredEvent
            })
        ));
        let state = lock(&state);
        assert!(state.generated_products.is_empty());
        assert_eq!(0, state.begins);
    }

    #[tokio::test]
    async fn should_skip_stale_ignored_and_missing_title_before_generation_or_transaction() {
        for outcome_case in ["stale", "ignored", "missing"] {
            let state = state();
            match outcome_case {
                "stale" => {
                    lock(&state)
                        .source
                        .as_mut()
                        .unwrap_or_else(|| panic!("test source missing"))
                        .current_event_id = EventId::new()
                }
                "ignored" => {
                    lock(&state)
                        .source
                        .as_mut()
                        .unwrap_or_else(|| panic!("test source missing"))
                        .event_type = "UNSUPPORTED_EVENT".to_owned()
                }
                _ => {
                    lock(&state)
                        .source
                        .as_mut()
                        .unwrap_or_else(|| panic!("test source missing"))
                        .title = None
                }
            }
            let result = handler(&state)
                .execute(&context(Principal::System), command(&state))
                .await;
            assert!(matches!(
                result,
                Ok(EmbedProductListingEventResult {
                    outcome: EmbedProductListingEventOutcome::Stale
                        | EmbedProductListingEventOutcome::IgnoredEvent
                        | EmbedProductListingEventOutcome::MissingTitle
                })
            ));
            let state = lock(&state);
            assert!(state.generated_products.is_empty());
            assert_eq!(0, state.begins);
        }
    }

    #[tokio::test]
    async fn should_not_open_transaction_when_generation_fails() {
        let state = state();
        lock(&state).generation_fails = true;
        let result = handler(&state)
            .execute(&context(Principal::System), command(&state))
            .await;
        assert!(matches!(
            result,
            Err(EmbedProductListingEventError::GenerationFailed { .. })
        ));
        assert_eq!(0, lock(&state).begins);
    }

    #[tokio::test]
    async fn should_commit_duplicate_and_stale_writer_outcomes() {
        for write_outcome in [
            ProductListingEmbeddingWriteOutcome::Duplicate,
            ProductListingEmbeddingWriteOutcome::Stale,
        ] {
            let state = state();
            lock(&state).write_outcome = write_outcome;
            let result = handler(&state)
                .execute(&context(Principal::System), command(&state))
                .await;
            assert!(matches!(
                result,
                Ok(EmbedProductListingEventResult {
                    outcome: EmbedProductListingEventOutcome::Duplicate
                        | EmbedProductListingEventOutcome::Stale
                })
            ));
            assert_eq!(1, lock(&state).commits);
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
        assert!(lock(&state).generated_products.is_empty());
    }
}
