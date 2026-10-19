use crate::ports::{
    PublicCatalogueOverviewReadError, PublicCatalogueOverviewReader,
    PublicCatalogueOverviewReaderFactory,
};
use application::{
    error::BoxError,
    operation_context::OperationContext,
    transaction::{Transaction, TransactionError, UnitOfWork},
};
use time::OffsetDateTime;

/// Aggregate public-catalogue statistics over `ACTIVE` ProductListings at `as_of`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicCatalogueOverview {
    pub as_of: OffsetDateTime,
    pub sources_indexed: u64,
    pub live_objects: u64,
    pub countries: u64,
    pub languages: u64,
    pub new_objects_last_7_days: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum GetPublicCatalogueOverviewError {
    #[error("temporary public catalogue overview reader failure")]
    ReaderTemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid public catalogue overview reader model")]
    ReaderInvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin public catalogue overview transaction")]
    BeginTransaction {
        #[source]
        source: TransactionError,
    },
    #[error("failed to commit public catalogue overview transaction")]
    CommitTransaction {
        #[source]
        source: TransactionError,
    },
}

#[async_trait::async_trait]
pub trait GetPublicCatalogueOverviewUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
    ) -> Result<PublicCatalogueOverview, GetPublicCatalogueOverviewError>;
}

pub struct GetPublicCatalogueOverviewHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> GetPublicCatalogueOverviewHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> GetPublicCatalogueOverviewUseCase for GetPublicCatalogueOverviewHandler<U, R>
where
    U: UnitOfWork,
    R: PublicCatalogueOverviewReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "get_public_catalogue_overview",
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
    ) -> Result<PublicCatalogueOverview, GetPublicCatalogueOverviewError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|source| GetPublicCatalogueOverviewError::BeginTransaction { source })?;
        let overview = self.reader.in_transaction(&mut tx).read_overview().await?;

        tx.commit()
            .await
            .map_err(|source| GetPublicCatalogueOverviewError::CommitTransaction { source })?;

        Ok(overview)
    }
}

impl From<PublicCatalogueOverviewReadError> for GetPublicCatalogueOverviewError {
    fn from(value: PublicCatalogueOverviewReadError) -> Self {
        match value {
            PublicCatalogueOverviewReadError::TemporarilyUnavailable { source } => {
                Self::ReaderTemporarilyUnavailable { source }
            }
            PublicCatalogueOverviewReadError::InvalidReadModel { source } => {
                Self::ReaderInvalidReadModel { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::{
        error::static_error,
        operation_context::{CorrelationId, Principal, RequestId},
    };
    use std::sync::{Arc, Mutex, MutexGuard};
    use time::macros::datetime;

    #[derive(Default)]
    struct State {
        begins: usize,
        commits: usize,
        reads: usize,
    }

    #[derive(Clone, Copy, Default)]
    struct Failures {
        begin: bool,
        read: bool,
        commit: bool,
    }

    struct FakeUnitOfWork {
        state: Arc<Mutex<State>>,
        failures: Failures,
    }

    struct FakeTransaction {
        state: Arc<Mutex<State>>,
        commit_fails: bool,
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            if self.commit_fails {
                return Err(TransactionError::CommitFailed(static_error(
                    "test transaction failure",
                )));
            }
            lock(&self.state).commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            if self.failures.begin {
                return Err(TransactionError::BeginFailed(static_error(
                    "test transaction failure",
                )));
            }
            lock(&self.state).begins += 1;
            Ok(FakeTransaction {
                state: Arc::clone(&self.state),
                commit_fails: self.failures.commit,
            })
        }
    }

    struct FakeReaderFactory {
        state: Arc<Mutex<State>>,
        fails: bool,
    }

    struct FakeReader {
        state: Arc<Mutex<State>>,
        fails: bool,
    }

    impl PublicCatalogueOverviewReaderFactory<FakeTransaction> for FakeReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl PublicCatalogueOverviewReader + 'tx {
            FakeReader {
                state: Arc::clone(&self.state),
                fails: self.fails,
            }
        }
    }

    #[async_trait::async_trait]
    impl PublicCatalogueOverviewReader for FakeReader {
        async fn read_overview(
            &mut self,
        ) -> Result<PublicCatalogueOverview, PublicCatalogueOverviewReadError> {
            lock(&self.state).reads += 1;
            if self.fails {
                return Err(PublicCatalogueOverviewReadError::TemporarilyUnavailable {
                    source: static_error("reader unavailable"),
                });
            }
            Ok(overview())
        }
    }

    fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
        match value.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn overview() -> PublicCatalogueOverview {
        PublicCatalogueOverview {
            as_of: datetime!(2026-10-19 12:00 UTC),
            sources_indexed: 2,
            live_objects: 5,
            countries: 1,
            languages: 3,
            new_objects_last_7_days: 4,
        }
    }

    async fn execute(
        state: &Arc<Mutex<State>>,
        failures: Failures,
    ) -> Result<PublicCatalogueOverview, GetPublicCatalogueOverviewError> {
        GetPublicCatalogueOverviewHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(state),
                failures,
            },
            FakeReaderFactory {
                state: Arc::clone(state),
                fails: failures.read,
            },
        )
        .execute(&OperationContext {
            principal: Principal::Anonymous,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        })
        .await
    }

    #[tokio::test]
    async fn should_read_anonymous_overview_in_one_committed_transaction() {
        let state = Arc::new(Mutex::new(State::default()));

        let result = execute(&state, Failures::default()).await;

        assert_eq!(overview(), result.unwrap_or_else(|error| panic!("{error}")));
        let state = lock(&state);
        assert_eq!((1, 1, 1), (state.begins, state.reads, state.commits));
    }

    #[tokio::test]
    async fn should_preserve_reader_failure_without_committing() {
        let state = Arc::new(Mutex::new(State::default()));

        let result = execute(
            &state,
            Failures {
                read: true,
                ..Failures::default()
            },
        )
        .await;

        assert!(matches!(
            result,
            Err(GetPublicCatalogueOverviewError::ReaderTemporarilyUnavailable { .. })
        ));
        assert_eq!(0, lock(&state).commits);
    }

    #[tokio::test]
    async fn should_preserve_transaction_failures() {
        let state = Arc::new(Mutex::new(State::default()));

        let begin = execute(
            &state,
            Failures {
                begin: true,
                ..Failures::default()
            },
        )
        .await;
        let commit = execute(
            &state,
            Failures {
                commit: true,
                ..Failures::default()
            },
        )
        .await;

        assert!(matches!(
            begin,
            Err(GetPublicCatalogueOverviewError::BeginTransaction { .. })
        ));
        assert!(matches!(
            commit,
            Err(GetPublicCatalogueOverviewError::CommitTransaction { .. })
        ));
        let state = lock(&state);
        assert_eq!((1, 1, 0), (state.begins, state.reads, state.commits));
    }
}
