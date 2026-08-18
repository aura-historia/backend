use crate::ports::{
    FxRateQuote, FxRateQuoteProvider, FxRateQuoteProviderError, FxRateSnapshotInsertOutcome,
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use common::{
    currency::domain::Currency,
    error::boxed::{BoxError, box_error},
    fx_rate_id::FxRateId,
    operation_context::{OperationAuthorizationError, OperationContext},
    transaction::{Transaction, UnitOfWork},
};
use fxrate_core::{
    FX_RATE_SCALE, FxRateGeneration, FxRateQuote as SnapshotQuote, FxRateSource, NewFxRateSnapshot,
};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureFxRateSnapshotCommand {
    pub source_event_id: String,
    pub captured_at: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureFxRateSnapshotOutcome {
    Captured {
        fx_rate_id: FxRateId,
        generation: FxRateGeneration,
    },
    Duplicate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureFxRateSnapshotResult {
    pub outcome: CaptureFxRateSnapshotOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureFxRateSnapshotError {
    #[error("service or system principal required for FX rate capture")]
    ServiceOrSystemPrincipalRequired,
    #[error("FX rate capture source event ID is required")]
    SourceEventIdRequired,
    #[error("failed to fetch FX rate quotes")]
    QuoteFetchFailed {
        #[source]
        source: BoxError,
    },
    #[error("FX rate quotes are invalid")]
    InvalidQuotes {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin FX rate snapshot transaction")]
    BeginTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to store FX rate snapshot")]
    SnapshotInsertFailed {
        #[source]
        source: BoxError,
    },
    #[error(
        "FX rate snapshot captured-at timestamp is not strictly newer than the canonical snapshot"
    )]
    CapturedAtNotMonotonic,
    #[error("failed to commit FX rate snapshot transaction")]
    CommitTransactionFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait CaptureFxRateSnapshotUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CaptureFxRateSnapshotCommand,
    ) -> Result<CaptureFxRateSnapshotResult, CaptureFxRateSnapshotError>;
}

pub struct CaptureFxRateSnapshotHandler<P, U, R> {
    quotes: P,
    unit_of_work: U,
    snapshots: R,
}

impl<P, U, R> CaptureFxRateSnapshotHandler<P, U, R> {
    pub fn new(quotes: P, unit_of_work: U, snapshots: R) -> Self {
        Self {
            quotes,
            unit_of_work,
            snapshots,
        }
    }
}

#[async_trait::async_trait]
impl<P, U, R> CaptureFxRateSnapshotUseCase for CaptureFxRateSnapshotHandler<P, U, R>
where
    P: FxRateQuoteProvider,
    U: UnitOfWork,
    R: FxRateSnapshotRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "capture_fx_rate_snapshot",
        skip_all,
        fields(
            source_event_id = %command.source_event_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CaptureFxRateSnapshotCommand,
    ) -> Result<CaptureFxRateSnapshotResult, CaptureFxRateSnapshotError> {
        context
            .require()
            .service_or_system()
            .authorize::<CaptureFxRateSnapshotError>()?;
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }
        if command.source_event_id.trim().is_empty() {
            return Err(CaptureFxRateSnapshotError::SourceEventIdRequired);
        }

        let quotes = self.quotes.fetch_eur_quotes().await.map_err(|source| {
            CaptureFxRateSnapshotError::QuoteFetchFailed {
                source: box_error(source),
            }
        })?;
        let snapshot = NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            command.captured_at,
            FxRateSource::FxRatesApi,
            quotes.base,
            std::iter::once(SnapshotQuote::new(Currency::Eur, FX_RATE_SCALE)).chain(
                quotes.quotes.into_iter().map(|quote: FxRateQuote| {
                    SnapshotQuote::new(quote.currency, quote.units_per_eur)
                }),
            ),
        )
        .map_err(|source| CaptureFxRateSnapshotError::InvalidQuotes {
            source: box_error(source),
        })?;

        let mut transaction = self.unit_of_work.begin().await.map_err(|source| {
            CaptureFxRateSnapshotError::BeginTransactionFailed {
                source: box_error(source),
            }
        })?;
        let outcome = self
            .snapshots
            .in_transaction(&mut transaction)
            .insert(&snapshot, &command.source_event_id)
            .await
            .map_err(CaptureFxRateSnapshotError::from)?;
        transaction.commit().await.map_err(|source| {
            CaptureFxRateSnapshotError::CommitTransactionFailed {
                source: box_error(source),
            }
        })?;

        let outcome = match outcome {
            FxRateSnapshotInsertOutcome::Inserted(snapshot) => {
                tracing::info!(
                    event = "fxrate.snapshot.captured",
                    actor_type = context.principal.kind(),
                    fx_rate_id = %snapshot.id(),
                    generation = snapshot.generation().as_i64(),
                    source_event_id = %command.source_event_id,
                    outcome = "success",
                );
                CaptureFxRateSnapshotOutcome::Captured {
                    fx_rate_id: snapshot.id(),
                    generation: snapshot.generation(),
                }
            }
            FxRateSnapshotInsertOutcome::Duplicate => CaptureFxRateSnapshotOutcome::Duplicate,
        };

        Ok(CaptureFxRateSnapshotResult { outcome })
    }
}

impl From<OperationAuthorizationError> for CaptureFxRateSnapshotError {
    fn from(_: OperationAuthorizationError) -> Self {
        Self::ServiceOrSystemPrincipalRequired
    }
}

impl From<FxRateQuoteProviderError> for CaptureFxRateSnapshotError {
    fn from(source: FxRateQuoteProviderError) -> Self {
        Self::QuoteFetchFailed {
            source: box_error(source),
        }
    }
}

impl From<FxRateSnapshotRepositoryError> for CaptureFxRateSnapshotError {
    fn from(source: FxRateSnapshotRepositoryError) -> Self {
        match source {
            FxRateSnapshotRepositoryError::CapturedAtNotMonotonic => Self::CapturedAtNotMonotonic,
            source => Self::SnapshotInsertFailed {
                source: box_error(source),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{FxRateQuoteSet, FxRateSnapshotRepository};
    use common::{
        error::boxed::static_error,
        operation_context::{CorrelationId, Principal, RequestId},
        transaction::TransactionError,
    };

    use std::sync::{Arc, Mutex};
    use strum::IntoEnumIterator;

    #[derive(Default)]
    struct State {
        fail_quotes: bool,
        duplicate: bool,
        begins: usize,
        commits: usize,
    }
    type SharedState = Arc<Mutex<State>>;
    struct Provider(SharedState);
    struct Uow(SharedState);
    struct Tx(SharedState);
    struct RepositoryFactory(SharedState);
    struct Repository(SharedState);

    fn lock(state: &SharedState) -> std::sync::MutexGuard<'_, State> {
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn command() -> CaptureFxRateSnapshotCommand {
        CaptureFxRateSnapshotCommand {
            source_event_id: "event-1".to_owned(),
            captured_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[async_trait::async_trait]
    impl FxRateQuoteProvider for Provider {
        async fn fetch_eur_quotes(&self) -> Result<FxRateQuoteSet, FxRateQuoteProviderError> {
            if lock(&self.0).fail_quotes {
                return Err(FxRateQuoteProviderError::RequestFailed {
                    source: static_error("test quote failure"),
                });
            }
            Ok(FxRateQuoteSet {
                base: Currency::Eur,
                quotes: Currency::iter()
                    .filter(|currency| *currency != Currency::Eur)
                    .map(|currency| FxRateQuote {
                        currency,
                        units_per_eur: 1_250_000,
                    })
                    .collect(),
            })
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for Uow {
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

    impl FxRateSnapshotRepositoryFactory<Tx> for RepositoryFactory {
        fn in_transaction<'tx>(&'tx self, _: &'tx mut Tx) -> impl FxRateSnapshotRepository + 'tx {
            Repository(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl FxRateSnapshotRepository for Repository {
        async fn find_latest(
            &mut self,
        ) -> Result<Option<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(None)
        }

        async fn find_latest_at_or_before(
            &mut self,
            _timestamp: OffsetDateTime,
        ) -> Result<Option<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(None)
        }

        async fn find_by_id(
            &mut self,
            _id: FxRateId,
        ) -> Result<Option<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(None)
        }

        async fn find_by_ids(
            &mut self,
            _ids: &[FxRateId],
        ) -> Result<Vec<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(Vec::new())
        }

        async fn insert(
            &mut self,
            snapshot: &NewFxRateSnapshot,
            _: &str,
        ) -> Result<FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError> {
            if lock(&self.0).duplicate {
                return Ok(FxRateSnapshotInsertOutcome::Duplicate);
            }
            let generation = FxRateGeneration::try_from(1).map_err(|source| {
                FxRateSnapshotRepositoryError::InsertFailed {
                    source: box_error(source),
                }
            })?;
            Ok(FxRateSnapshotInsertOutcome::Inserted(
                snapshot.clone().into_persisted(generation),
            ))
        }
    }

    #[tokio::test]
    async fn should_fetch_validate_store_and_commit_snapshot_for_system() {
        let state = Arc::new(Mutex::new(State::default()));
        let handler = CaptureFxRateSnapshotHandler::new(
            Provider(Arc::clone(&state)),
            Uow(Arc::clone(&state)),
            RepositoryFactory(Arc::clone(&state)),
        );

        let result = handler
            .execute(&context(Principal::System), command())
            .await;

        assert!(
            matches!(result, Ok(CaptureFxRateSnapshotResult { outcome: CaptureFxRateSnapshotOutcome::Captured { generation, .. } }) if generation.as_i64() == 1)
        );
        assert_eq!(1, lock(&state).begins);
        assert_eq!(1, lock(&state).commits);
    }

    #[tokio::test]
    async fn should_not_open_transaction_when_quote_fetch_fails_or_actor_is_unauthorized() {
        let failed = Arc::new(Mutex::new(State {
            fail_quotes: true,
            ..Default::default()
        }));
        let handler = CaptureFxRateSnapshotHandler::new(
            Provider(Arc::clone(&failed)),
            Uow(Arc::clone(&failed)),
            RepositoryFactory(Arc::clone(&failed)),
        );
        assert!(matches!(
            handler
                .execute(&context(Principal::System), command())
                .await,
            Err(CaptureFxRateSnapshotError::QuoteFetchFailed { .. })
        ));
        assert_eq!(0, lock(&failed).begins);

        let unauthorized = Arc::new(Mutex::new(State::default()));
        let handler = CaptureFxRateSnapshotHandler::new(
            Provider(Arc::clone(&unauthorized)),
            Uow(Arc::clone(&unauthorized)),
            RepositoryFactory(Arc::clone(&unauthorized)),
        );
        assert!(matches!(
            handler
                .execute(
                    &context(Principal::User(common::user_id::UserId::new())),
                    command()
                )
                .await,
            Err(CaptureFxRateSnapshotError::ServiceOrSystemPrincipalRequired)
        ));
        assert_eq!(0, lock(&unauthorized).begins);
    }

    #[tokio::test]
    async fn should_commit_duplicate_capture_without_reporting_a_new_generation() {
        let state = Arc::new(Mutex::new(State {
            duplicate: true,
            ..Default::default()
        }));
        let handler = CaptureFxRateSnapshotHandler::new(
            Provider(Arc::clone(&state)),
            Uow(Arc::clone(&state)),
            RepositoryFactory(Arc::clone(&state)),
        );

        assert!(matches!(
            handler
                .execute(&context(Principal::System), command())
                .await,
            Ok(CaptureFxRateSnapshotResult {
                outcome: CaptureFxRateSnapshotOutcome::Duplicate
            })
        ));
        assert_eq!(1, lock(&state).commits);
    }
}
