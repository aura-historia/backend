use crate::ports::{
    ProductTitleTranslationError, ProductTitleTranslator, ProductTranslationSourceReadError,
    ProductTranslationSourceReader, ProductTranslationWrite, ProductTranslationWriteError,
    ProductTranslationWriteOutcome, ProductTranslationWriter, ProductTranslationWriterFactory,
};
use common::{
    error::boxed::{BoxError, box_error},
    event_id::EventId,
    operation_context::{OperationAuthorizationError, OperationContext},
    product_id::ProductId,
    transaction::{Transaction, UnitOfWork},
};
use localization::Language;
const EMBEDDED_EVENT_TYPE: &str = "ENRICHMENT_EMBEDDED";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranslateProductCommand {
    pub event_id: EventId,
    pub product_id: ProductId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranslateProductEventOutcome {
    Applied,
    Duplicate,
    Stale,
    ProductNotFound,
    IgnoredEvent,
    MissingTitle,
    MissingTitleLanguage,
    EmptyTitle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranslateProductEventResult {
    pub outcome: TranslateProductEventOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum TranslateProductEventError {
    #[error("service or system principal required for product translation")]
    ServiceOrSystemPrincipalRequired,
    #[error("failed to read product translation source")]
    SourceReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("product title translation failed")]
    TranslationFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin product translation transaction")]
    BeginTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to persist product translations")]
    TranslationWriteFailed {
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
pub trait TranslateProductEventUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: TranslateProductCommand,
    ) -> Result<TranslateProductEventResult, TranslateProductEventError>;
}

pub struct TranslateProductEventHandler<S, T, U, W> {
    sources: S,
    translator: T,
    unit_of_work: U,
    translations: W,
}

impl<S, T, U, W> TranslateProductEventHandler<S, T, U, W> {
    pub fn new(sources: S, translator: T, unit_of_work: U, translations: W) -> Self {
        Self {
            sources,
            translator,
            unit_of_work,
            translations,
        }
    }
}

#[async_trait::async_trait]
impl<S, T, U, W> TranslateProductEventUseCase for TranslateProductEventHandler<S, T, U, W>
where
    S: ProductTranslationSourceReader,
    T: ProductTitleTranslator,
    U: UnitOfWork,
    W: ProductTranslationWriterFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "translate_product_event",
        skip_all,
        fields(
            product_id = %command.product_id,
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
        command: TranslateProductCommand,
    ) -> Result<TranslateProductEventResult, TranslateProductEventError> {
        context
            .require()
            .service_or_system()
            .authorize::<TranslateProductEventError>()?;
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }

        let Some(source) = self
            .sources
            .find_source(command.event_id, command.product_id)
            .await
            .map_err(|source| TranslateProductEventError::SourceReadFailed {
                source: box_error(source),
            })?
        else {
            return Ok(result(TranslateProductEventOutcome::ProductNotFound));
        };

        if source.current_event_id != command.event_id {
            return Ok(result(TranslateProductEventOutcome::Stale));
        }
        if source.event_type != EMBEDDED_EVENT_TYPE {
            return Ok(result(TranslateProductEventOutcome::IgnoredEvent));
        }
        let Some(title) = source.title else {
            return Ok(result(TranslateProductEventOutcome::MissingTitle));
        };
        let Some(source_language) = source.title_language else {
            return Ok(result(TranslateProductEventOutcome::MissingTitleLanguage));
        };
        if title.as_ref().is_empty() {
            return Ok(result(TranslateProductEventOutcome::EmptyTitle));
        }

        let target_languages = translation_targets(source_language);
        let titles = self
            .translator
            .translate(&title, source_language, &target_languages)
            .await
            .map_err(|source| TranslateProductEventError::TranslationFailed {
                source: box_error(source),
            })?;

        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            TranslateProductEventError::BeginTransactionFailed {
                source: box_error(source),
            }
        })?;
        let outcome = self
            .translations
            .in_transaction(&mut tx)
            .apply(&ProductTranslationWrite {
                product_id: command.product_id,
                source_event_id: command.event_id,
                enrichment_event_id: EventId::new(),
                source_language,
                titles,
            })
            .await
            .map_err(
                |source| TranslateProductEventError::TranslationWriteFailed {
                    source: box_error(source),
                },
            )?;
        tx.commit().await.map_err(|source| {
            TranslateProductEventError::CommitTransactionFailed {
                source: box_error(source),
            }
        })?;

        let outcome = match outcome {
            ProductTranslationWriteOutcome::Applied => TranslateProductEventOutcome::Applied,
            ProductTranslationWriteOutcome::Duplicate => TranslateProductEventOutcome::Duplicate,
            ProductTranslationWriteOutcome::Stale => TranslateProductEventOutcome::Stale,
            ProductTranslationWriteOutcome::ProductNotFound => {
                TranslateProductEventOutcome::ProductNotFound
            }
        };
        if outcome == TranslateProductEventOutcome::Applied {
            tracing::info!(
                event = "product.translated",
                actor_type = context.principal.kind(),
                product_id = %command.product_id,
                source_event_id = %command.event_id,
                outcome = "success",
            );
        }
        Ok(result(outcome))
    }
}

fn result(outcome: TranslateProductEventOutcome) -> TranslateProductEventResult {
    TranslateProductEventResult { outcome }
}

fn translation_targets(source_language: Language) -> Vec<Language> {
    [
        Language::De,
        Language::En,
        Language::Fr,
        Language::Es,
        Language::It,
    ]
    .into_iter()
    .filter(|language| *language != source_language)
    .collect()
}

impl From<OperationAuthorizationError> for TranslateProductEventError {
    fn from(_error: OperationAuthorizationError) -> Self {
        Self::ServiceOrSystemPrincipalRequired
    }
}

impl From<ProductTranslationSourceReadError> for TranslateProductEventError {
    fn from(source: ProductTranslationSourceReadError) -> Self {
        Self::SourceReadFailed {
            source: box_error(source),
        }
    }
}

impl From<ProductTitleTranslationError> for TranslateProductEventError {
    fn from(source: ProductTitleTranslationError) -> Self {
        Self::TranslationFailed {
            source: box_error(source),
        }
    }
}

impl From<ProductTranslationWriteError> for TranslateProductEventError {
    fn from(source: ProductTranslationWriteError) -> Self {
        Self::TranslationWriteFailed {
            source: box_error(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::{
        operation_context::{CorrelationId, Principal, RequestId},
        transaction::TransactionError,
    };
    use indexmap::IndexMap;
    use product_core::title::Title;
    use std::sync::{Arc, Mutex};

    struct State {
        source: Option<crate::ports::ProductTranslationSource>,
        source_error: bool,
        translation_error: bool,
        write_outcome: ProductTranslationWriteOutcome,
        begin_error: bool,
        commit_error: bool,
        translations: Vec<(Language, String)>,
        begins: usize,
        commits: usize,
    }

    impl Default for State {
        fn default() -> Self {
            Self {
                source: None,
                source_error: false,
                translation_error: false,
                write_outcome: ProductTranslationWriteOutcome::Applied,
                begin_error: false,
                commit_error: false,
                translations: Vec::new(),
                begins: 0,
                commits: 0,
            }
        }
    }

    type SharedState = Arc<Mutex<State>>;

    #[derive(Clone)]
    struct Sources(SharedState);
    #[derive(Clone)]
    struct Translator(SharedState);
    #[derive(Clone)]
    struct UnitOfWorkFake(SharedState);
    #[derive(Clone)]
    struct WriterFactory(SharedState);
    struct Tx(SharedState);
    struct Writer(SharedState);

    fn lock(state: &SharedState) -> std::sync::MutexGuard<'_, State> {
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[async_trait::async_trait]
    impl ProductTranslationSourceReader for Sources {
        async fn find_source(
            &self,
            _event_id: EventId,
            _product_id: ProductId,
        ) -> Result<Option<crate::ports::ProductTranslationSource>, ProductTranslationSourceReadError>
        {
            let state = lock(&self.0);
            if state.source_error {
                return Err(ProductTranslationSourceReadError::QueryFailed {
                    source: box_error(std::io::Error::other("source failed")),
                });
            }
            Ok(state.source.clone())
        }
    }

    #[async_trait::async_trait]
    impl ProductTitleTranslator for Translator {
        async fn translate(
            &self,
            _title: &product_core::title::Title,
            _source_language: Language,
            target_languages: &[Language],
        ) -> Result<IndexMap<Language, product_core::title::Title>, ProductTitleTranslationError>
        {
            let mut state = lock(&self.0);
            if state.translation_error {
                return Err(ProductTitleTranslationError::TemporarilyUnavailable {
                    source: box_error(std::io::Error::other("translation failed")),
                });
            }
            state.translations = target_languages
                .iter()
                .map(|language| (*language, format!("{} title", language.as_str())))
                .collect();
            Ok(target_languages
                .iter()
                .map(|language| {
                    (
                        *language,
                        Title::from(format!("{} title", language.as_str())),
                    )
                })
                .collect())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for UnitOfWorkFake {
        type Tx = Tx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let mut state = lock(&self.0);
            state.begins += 1;
            if state.begin_error {
                Err(TransactionError::BeginFailed)
            } else {
                Ok(Tx(Arc::clone(&self.0)))
            }
        }
    }

    #[async_trait::async_trait]
    impl Transaction for Tx {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = lock(&self.0);
            state.commits += 1;
            if state.commit_error {
                Err(TransactionError::CommitFailed)
            } else {
                Ok(())
            }
        }
    }

    impl ProductTranslationWriterFactory<Tx> for WriterFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut Tx) -> impl ProductTranslationWriter + 'tx {
            Writer(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl ProductTranslationWriter for Writer {
        async fn apply(
            &mut self,
            _write: &ProductTranslationWrite,
        ) -> Result<ProductTranslationWriteOutcome, ProductTranslationWriteError> {
            Ok(lock(&self.0).write_outcome)
        }
    }

    fn state_with_source() -> SharedState {
        let product_id = ProductId::new();
        let event_id = EventId::new();
        Arc::new(Mutex::new(State {
            source: Some(crate::ports::ProductTranslationSource {
                product_id,
                event_id,
                current_event_id: event_id,
                event_type: EMBEDDED_EVENT_TYPE.to_owned(),
                title: Some(Title::from("Ancient vase")),
                title_language: Some(Language::De),
            }),
            write_outcome: ProductTranslationWriteOutcome::Applied,
            ..Default::default()
        }))
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
    ) -> TranslateProductEventHandler<Sources, Translator, UnitOfWorkFake, WriterFactory> {
        TranslateProductEventHandler::new(
            Sources(Arc::clone(state)),
            Translator(Arc::clone(state)),
            UnitOfWorkFake(Arc::clone(state)),
            WriterFactory(Arc::clone(state)),
        )
    }

    fn command(state: &SharedState) -> TranslateProductCommand {
        let source = lock(state).source.clone().expect("source exists");
        TranslateProductCommand {
            event_id: source.event_id,
            product_id: source.product_id,
        }
    }

    #[tokio::test]
    async fn should_translate_all_supported_target_languages_and_commit() {
        let state = state_with_source();

        let result = handler(&state)
            .execute(&context(Principal::System), command(&state))
            .await;

        assert!(matches!(
            result,
            Ok(TranslateProductEventResult {
                outcome: TranslateProductEventOutcome::Applied
            })
        ));
        let state = lock(&state);
        assert_eq!(4, state.translations.len());
        assert!(
            !state
                .translations
                .iter()
                .any(|(language, _)| *language == Language::De)
        );
        assert_eq!(1, state.commits);
    }

    #[tokio::test]
    async fn should_skip_stale_event_without_translating_or_starting_transaction() {
        let state = state_with_source();
        lock(&state)
            .source
            .as_mut()
            .expect("source exists")
            .current_event_id = EventId::new();

        let result = handler(&state)
            .execute(&context(Principal::System), command(&state))
            .await;

        assert!(matches!(
            result,
            Ok(TranslateProductEventResult {
                outcome: TranslateProductEventOutcome::Stale
            })
        ));
        let state = lock(&state);
        assert!(state.translations.is_empty());
        assert_eq!(0, state.commits);
    }

    #[tokio::test]
    async fn should_skip_non_embedded_event_without_side_effect() {
        let state = state_with_source();
        lock(&state)
            .source
            .as_mut()
            .expect("source exists")
            .event_type = "PRODUCT_CREATED".to_owned();

        let result = handler(&state)
            .execute(&context(Principal::System), command(&state))
            .await;

        assert!(matches!(
            result,
            Ok(TranslateProductEventResult {
                outcome: TranslateProductEventOutcome::IgnoredEvent
            })
        ));
        assert_eq!(0, lock(&state).commits);
    }

    #[tokio::test]
    async fn should_return_duplicate_after_target_write_reports_duplicate() {
        let state = state_with_source();
        lock(&state).write_outcome = ProductTranslationWriteOutcome::Duplicate;

        let result = handler(&state)
            .execute(&context(Principal::System), command(&state))
            .await;

        assert!(matches!(
            result,
            Ok(TranslateProductEventResult {
                outcome: TranslateProductEventOutcome::Duplicate
            })
        ));
        assert_eq!(1, lock(&state).commits);
    }

    #[tokio::test]
    async fn should_skip_source_without_title() {
        let state = state_with_source();
        lock(&state).source.as_mut().expect("source exists").title = None;
        lock(&state)
            .source
            .as_mut()
            .expect("source exists")
            .title_language = None;

        let result = handler(&state)
            .execute(&context(Principal::System), command(&state))
            .await;

        assert!(matches!(
            result,
            Ok(TranslateProductEventResult {
                outcome: TranslateProductEventOutcome::MissingTitle
            })
        ));
        assert_eq!(0, lock(&state).commits);
    }

    #[tokio::test]
    async fn should_skip_source_without_title_language() {
        let state = state_with_source();
        lock(&state)
            .source
            .as_mut()
            .expect("source exists")
            .title_language = None;

        let result = handler(&state)
            .execute(&context(Principal::System), command(&state))
            .await;

        assert!(matches!(
            result,
            Ok(TranslateProductEventResult {
                outcome: TranslateProductEventOutcome::MissingTitleLanguage
            })
        ));
        assert_eq!(0, lock(&state).commits);
    }

    #[tokio::test]
    async fn should_skip_empty_title_without_translating() {
        let state = state_with_source();
        lock(&state).source.as_mut().expect("source exists").title = Some(Title::from(""));

        let result = handler(&state)
            .execute(&context(Principal::System), command(&state))
            .await;

        assert!(matches!(
            result,
            Ok(TranslateProductEventResult {
                outcome: TranslateProductEventOutcome::EmptyTitle
            })
        ));
        assert_eq!(0, lock(&state).commits);
    }

    #[tokio::test]
    async fn should_not_begin_transaction_when_translation_fails() {
        let state = state_with_source();
        lock(&state).translation_error = true;

        let result = handler(&state)
            .execute(&context(Principal::System), command(&state))
            .await;

        assert!(matches!(
            result,
            Err(TranslateProductEventError::TranslationFailed { .. })
        ));
        let state = lock(&state);
        assert_eq!(0, state.begins);
        assert_eq!(0, state.commits);
    }

    #[tokio::test]
    async fn should_reject_user_principal() {
        let state = state_with_source();

        let result = handler(&state)
            .execute(
                &context(common::operation_context::Principal::User(
                    common::user_id::UserId::new(),
                )),
                command(&state),
            )
            .await;

        assert!(matches!(
            result,
            Err(TranslateProductEventError::ServiceOrSystemPrincipalRequired)
        ));
        assert_eq!(0, lock(&state).commits);
    }
}
