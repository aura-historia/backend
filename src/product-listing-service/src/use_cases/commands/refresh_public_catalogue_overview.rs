use crate::{
    ports::{
        PublicCatalogueOverviewRefreshError, PublicCatalogueOverviewRefresher,
        PublicCatalogueOverviewRefresherFactory,
    },
    use_cases::queries::get_public_catalogue_overview::PublicCatalogueOverview,
};
use application::{
    error::BoxError,
    transaction::{Transaction, TransactionError, UnitOfWork},
};
use std::time::Duration;

/// The hourly maintenance run recalculates once the stored calculation reaches this age,
/// so the full aggregate runs about every six hours despite small start-time jitter.
pub const PUBLIC_CATALOGUE_OVERVIEW_REFRESH_AGE: Duration = Duration::from_secs(5 * 3600 + 45 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicCatalogueOverviewRefresh {
    pub overview: PublicCatalogueOverview,
    /// `false` when a concurrent refresh already stored a newer calculation.
    pub stored: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum RefreshPublicCatalogueOverviewError {
    #[error("temporary public catalogue overview refresh failure")]
    RefresherTemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid public catalogue overview calculation")]
    RefresherInvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin public catalogue overview refresh transaction")]
    BeginTransaction {
        #[source]
        source: TransactionError,
    },
    #[error("failed to commit public catalogue overview refresh transaction")]
    CommitTransaction {
        #[source]
        source: TransactionError,
    },
}

/// Scheduled maintenance: recalculates the stored public catalogue statistics once they
/// reach [`PUBLIC_CATALOGUE_OVERVIEW_REFRESH_AGE`]; `None` means the stored ones are current.
#[async_trait::async_trait]
pub trait RefreshPublicCatalogueOverviewUseCase: Send + Sync {
    async fn execute(
        &self,
    ) -> Result<Option<PublicCatalogueOverviewRefresh>, RefreshPublicCatalogueOverviewError>;
}

pub struct RefreshPublicCatalogueOverviewHandler<U, R> {
    unit_of_work: U,
    refresher: R,
}

impl<U, R> RefreshPublicCatalogueOverviewHandler<U, R> {
    pub fn new(unit_of_work: U, refresher: R) -> Self {
        Self {
            unit_of_work,
            refresher,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> RefreshPublicCatalogueOverviewUseCase for RefreshPublicCatalogueOverviewHandler<U, R>
where
    U: UnitOfWork,
    R: PublicCatalogueOverviewRefresherFactory<U::Tx>,
{
    #[tracing::instrument(name = "refresh_public_catalogue_overview", skip_all)]
    async fn execute(
        &self,
    ) -> Result<Option<PublicCatalogueOverviewRefresh>, RefreshPublicCatalogueOverviewError> {
        let mut tx =
            self.unit_of_work.begin().await.map_err(|source| {
                RefreshPublicCatalogueOverviewError::BeginTransaction { source }
            })?;
        let refresh = self
            .refresher
            .in_transaction(&mut tx)
            .refresh_if_older_than(PUBLIC_CATALOGUE_OVERVIEW_REFRESH_AGE)
            .await?;

        tx.commit()
            .await
            .map_err(|source| RefreshPublicCatalogueOverviewError::CommitTransaction { source })?;

        Ok(refresh)
    }
}

impl From<PublicCatalogueOverviewRefreshError> for RefreshPublicCatalogueOverviewError {
    fn from(value: PublicCatalogueOverviewRefreshError) -> Self {
        match value {
            PublicCatalogueOverviewRefreshError::TemporarilyUnavailable { source } => {
                Self::RefresherTemporarilyUnavailable { source }
            }
            PublicCatalogueOverviewRefreshError::InvalidReadModel { source } => {
                Self::RefresherInvalidReadModel { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::static_error;
    use std::sync::{Arc, Mutex, MutexGuard};
    use time::macros::datetime;

    #[derive(Default)]
    struct State {
        begins: usize,
        refreshes: usize,
        commits: usize,
        max_age: Option<Duration>,
    }

    struct FakeUnitOfWork {
        state: Arc<Mutex<State>>,
    }

    struct FakeTransaction {
        state: Arc<Mutex<State>>,
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            lock(&self.state).commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            lock(&self.state).begins += 1;
            Ok(FakeTransaction {
                state: Arc::clone(&self.state),
            })
        }
    }

    #[derive(Clone, Copy)]
    enum Outcome {
        Refreshed,
        Current,
        Fails,
    }

    struct FakeRefresherFactory {
        state: Arc<Mutex<State>>,
        outcome: Outcome,
    }

    struct FakeRefresher {
        state: Arc<Mutex<State>>,
        outcome: Outcome,
    }

    impl PublicCatalogueOverviewRefresherFactory<FakeTransaction> for FakeRefresherFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl PublicCatalogueOverviewRefresher + 'tx {
            FakeRefresher {
                state: Arc::clone(&self.state),
                outcome: self.outcome,
            }
        }
    }

    #[async_trait::async_trait]
    impl PublicCatalogueOverviewRefresher for FakeRefresher {
        async fn refresh_if_older_than(
            &mut self,
            max_age: Duration,
        ) -> Result<Option<PublicCatalogueOverviewRefresh>, PublicCatalogueOverviewRefreshError>
        {
            let mut state = lock(&self.state);
            state.refreshes += 1;
            state.max_age = Some(max_age);
            match self.outcome {
                Outcome::Refreshed => Ok(Some(refresh())),
                Outcome::Current => Ok(None),
                Outcome::Fails => Err(
                    PublicCatalogueOverviewRefreshError::TemporarilyUnavailable {
                        source: static_error("statement timeout"),
                    },
                ),
            }
        }
    }

    fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
        match value.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn refresh() -> PublicCatalogueOverviewRefresh {
        PublicCatalogueOverviewRefresh {
            overview: PublicCatalogueOverview {
                as_of: datetime!(2026-10-19 12:00 UTC),
                sources_indexed: 2,
                live_objects: 5,
                countries: 1,
                languages: 3,
                new_objects_last_7_days: 4,
            },
            stored: true,
        }
    }

    async fn execute(
        state: &Arc<Mutex<State>>,
        outcome: Outcome,
    ) -> Result<Option<PublicCatalogueOverviewRefresh>, RefreshPublicCatalogueOverviewError> {
        RefreshPublicCatalogueOverviewHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(state),
            },
            FakeRefresherFactory {
                state: Arc::clone(state),
                outcome,
            },
        )
        .execute()
        .await
    }

    #[tokio::test]
    async fn should_commit_due_refresh_or_current_calculation_check() {
        for (outcome, expected) in [
            (Outcome::Refreshed, Some(refresh())),
            (Outcome::Current, None),
        ] {
            let state = Arc::new(Mutex::new(State::default()));

            let result = execute(&state, outcome).await;

            assert_eq!(expected, result.unwrap_or_else(|error| panic!("{error}")));
            let state = lock(&state);
            assert_eq!((1, 1, 1), (state.begins, state.refreshes, state.commits));
            assert_eq!(Some(PUBLIC_CATALOGUE_OVERVIEW_REFRESH_AGE), state.max_age);
        }
        assert_eq!(
            Duration::from_secs(20_700),
            PUBLIC_CATALOGUE_OVERVIEW_REFRESH_AGE
        );
    }

    #[tokio::test]
    async fn should_preserve_refresh_failure_without_committing() {
        let state = Arc::new(Mutex::new(State::default()));

        let result = execute(&state, Outcome::Fails).await;

        assert!(matches!(
            result,
            Err(RefreshPublicCatalogueOverviewError::RefresherTemporarilyUnavailable { .. })
        ));
        assert_eq!(0, lock(&state).commits);
    }
}
