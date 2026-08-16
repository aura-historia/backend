use crate::ports::{
    CompiledSearchFilterProjection, SearchFilterIndex, SearchFilterIndexError,
    SearchFilterIndexReadError, SearchFilterIndexReader, SearchFilterProjectionWriteOutcome,
};
use common::error::boxed::{BoxError, box_error};
use common::transaction::{Transaction, UnitOfWork};
use common::user_search_filter_id::UserSearchFilterId;
use fxrate_core::FxRateSnapshot;
use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use product_service::ports::ProductPriceFilterPlan;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFilterProjectionOperation {
    Upsert,
    Delete,
}

impl SearchFilterProjectionOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Upsert => "upsert",
            Self::Delete => "delete",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSearchFilterChangeCommand {
    pub search_filter_id: UserSearchFilterId,
    pub source_version: i64,
    pub operation: SearchFilterProjectionOperation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSearchFilterChangeResult {
    pub outcome: SearchFilterProjectionWriteOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectSearchFilterChangeError {
    #[error("search filter projection source version must be positive")]
    InvalidSourceVersion,
    #[error("search filter projection delete version overflowed")]
    DeleteVersionOverflow,
    #[error("search filter projection read failed")]
    ReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter projection state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("latest FX rate snapshot is missing for search filter projection")]
    FxRateSnapshotMissing,
    #[error("failed to begin latest FX rate snapshot transaction for search filter projection")]
    BeginFxRateSnapshotTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("latest FX rate snapshot read failed for search filter projection")]
    FxRateSnapshotReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("latest FX rate snapshot is invalid for search filter projection")]
    FxRateSnapshotInvalid {
        #[source]
        source: BoxError,
    },
    #[error("failed to commit latest FX rate snapshot transaction for search filter projection")]
    CommitFxRateSnapshotTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter price filter could not be compiled")]
    PriceFilterCompilationFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter projection write failed")]
    WriteFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProjectSearchFilterChangeUseCase: Send + Sync {
    async fn execute(
        &self,
        command: ProjectSearchFilterChangeCommand,
    ) -> Result<ProjectSearchFilterChangeResult, ProjectSearchFilterChangeError>;
}

pub struct ProjectSearchFilterChangeHandler<U, R, F, I> {
    unit_of_work: U,
    source: R,
    fx_rates: F,
    index: I,
}

impl<U, R, F, I> ProjectSearchFilterChangeHandler<U, R, F, I> {
    pub fn new(unit_of_work: U, source: R, fx_rates: F, index: I) -> Self {
        Self {
            unit_of_work,
            source,
            fx_rates,
            index,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, F, I> ProjectSearchFilterChangeUseCase for ProjectSearchFilterChangeHandler<U, R, F, I>
where
    U: UnitOfWork,
    R: SearchFilterIndexReader,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
    I: SearchFilterIndex,
{
    #[tracing::instrument(
        name = "project_search_filter_change",
        skip_all,
        fields(
            search_filter_id = %command.search_filter_id,
            source_version = command.source_version,
            operation = command.operation.as_str(),
        )
    )]
    async fn execute(
        &self,
        command: ProjectSearchFilterChangeCommand,
    ) -> Result<ProjectSearchFilterChangeResult, ProjectSearchFilterChangeError> {
        if command.source_version <= 0 {
            return Err(ProjectSearchFilterChangeError::InvalidSourceVersion);
        }

        let outcome = match command.operation {
            SearchFilterProjectionOperation::Delete => self
                .index
                .delete(
                    command.search_filter_id,
                    tombstone_version(command.source_version)?,
                )
                .await
                .map_err(index_error)?,
            SearchFilterProjectionOperation::Upsert => match self
                .source
                .find_by_id(command.search_filter_id)
                .await
                .map_err(read_error)?
            {
                Some(projection) => {
                    let snapshot =
                        load_latest_fx_rate_snapshot(&self.unit_of_work, &self.fx_rates).await?;
                    let price_filter = ProductPriceFilterPlan::compile(
                        snapshot,
                        projection.view.search.currency,
                        projection.view.search.price_query,
                    )
                    .map_err(|source| {
                        ProjectSearchFilterChangeError::PriceFilterCompilationFailed {
                            source: box_error(source),
                        }
                    })?;
                    let compiled_projection = CompiledSearchFilterProjection {
                        projection,
                        price_filter_plan: price_filter,
                    };
                    self.index
                        .upsert(&compiled_projection)
                        .await
                        .map_err(index_error)?
                }
                None => self
                    .index
                    .delete(
                        command.search_filter_id,
                        tombstone_version(command.source_version)?,
                    )
                    .await
                    .map_err(index_error)?,
            },
        };

        Ok(ProjectSearchFilterChangeResult { outcome })
    }
}

fn tombstone_version(source_version: i64) -> Result<i64, ProjectSearchFilterChangeError> {
    source_version
        .checked_add(1)
        .ok_or(ProjectSearchFilterChangeError::DeleteVersionOverflow)
}

fn read_error(error: SearchFilterIndexReadError) -> ProjectSearchFilterChangeError {
    match error {
        SearchFilterIndexReadError::InvalidPersistedState { source } => {
            ProjectSearchFilterChangeError::InvalidPersistedState { source }
        }
        error => ProjectSearchFilterChangeError::ReadFailed {
            source: box_error(error),
        },
    }
}

async fn load_latest_fx_rate_snapshot<U, F>(
    unit_of_work: &U,
    fx_rates: &F,
) -> Result<FxRateSnapshot, ProjectSearchFilterChangeError>
where
    U: UnitOfWork,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
{
    let mut tx = unit_of_work.begin().await.map_err(|source| {
        ProjectSearchFilterChangeError::BeginFxRateSnapshotTransactionFailed {
            source: box_error(source),
        }
    })?;
    let snapshot = fx_rates
        .in_transaction(&mut tx)
        .find_latest()
        .await
        .map_err(fx_rate_snapshot_read_error)?;
    tx.commit().await.map_err(|source| {
        ProjectSearchFilterChangeError::CommitFxRateSnapshotTransactionFailed {
            source: box_error(source),
        }
    })?;
    snapshot.ok_or(ProjectSearchFilterChangeError::FxRateSnapshotMissing)
}

fn fx_rate_snapshot_read_error(
    error: FxRateSnapshotRepositoryError,
) -> ProjectSearchFilterChangeError {
    match error {
        FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { source } => {
            ProjectSearchFilterChangeError::FxRateSnapshotInvalid { source }
        }
        FxRateSnapshotRepositoryError::InsertFailed { source }
        | FxRateSnapshotRepositoryError::ReadFailed { source } => {
            ProjectSearchFilterChangeError::FxRateSnapshotReadFailed { source }
        }
    }
}

fn index_error(error: SearchFilterIndexError) -> ProjectSearchFilterChangeError {
    ProjectSearchFilterChangeError::WriteFailed {
        source: box_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{SearchFilterIndexQuery, SearchFilterProjection, SearchFilterView};
    use common::currency::domain::Currency;
    use common::fx_rate_id::FxRateId;
    use common::language::domain::Language;
    use common::pagination::cursor::CursoredResult;
    use common::resource_state::domain::ResourceState;
    use common::transaction::{TransactionError, UnitOfWork};
    use common::user_id::UserId;
    use common::user_search_filter_name::UserSearchFilterName;
    use fxrate_core::{
        FX_RATE_SCALE, FxRateQuote, FxRateSnapshot, FxRateSource, NewFxRateSnapshot,
    };
    use fxrate_service::ports::FxRateSnapshotRepositoryFactory;
    use product_core::product_search::ProductSearch;
    use std::sync::Mutex;
    use strum::IntoEnumIterator;
    use time::macros::datetime;

    #[derive(Default)]
    struct Source {
        projection: Mutex<Option<SearchFilterProjection>>,
    }

    #[async_trait::async_trait]
    impl SearchFilterIndexReader for Source {
        async fn find_by_id(
            &self,
            _search_filter_id: UserSearchFilterId,
        ) -> Result<Option<SearchFilterProjection>, SearchFilterIndexReadError> {
            self.projection
                .lock()
                .map_err(|_| SearchFilterIndexReadError::ReadFailed {
                    source: box_error(std::io::Error::other("test mutex poisoned")),
                })
                .map(|projection| projection.clone())
        }

        async fn list_after(
            &self,
            _after: Option<UserSearchFilterId>,
            _limit: usize,
        ) -> Result<Vec<SearchFilterProjection>, SearchFilterIndexReadError> {
            Ok(Vec::new())
        }
    }

    #[derive(Clone)]
    struct LatestFxRates {
        response: LatestFxRateResponse,
        transaction_state: std::sync::Arc<Mutex<FxRateTransactionState>>,
    }

    #[derive(Clone)]
    enum LatestFxRateResponse {
        Found(FxRateSnapshot),
        Missing,
        Invalid,
        ReadFailed,
    }

    #[derive(Default)]
    struct FxRateTransactionState {
        begin_error: bool,
        commit_error: bool,
        commit_count: usize,
    }

    struct FxRateTransaction {
        state: std::sync::Arc<Mutex<FxRateTransactionState>>,
    }

    struct FxRateSnapshotRepositoryFake {
        response: LatestFxRateResponse,
    }

    impl LatestFxRates {
        fn new(response: LatestFxRateResponse) -> Self {
            Self {
                response,
                transaction_state: std::sync::Arc::new(Mutex::new(
                    FxRateTransactionState::default(),
                )),
            }
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for LatestFxRates {
        type Tx = FxRateTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let state = self
                .transaction_state
                .lock()
                .map_err(|_| TransactionError::BeginFailed)?;
            if state.begin_error {
                return Err(TransactionError::BeginFailed);
            }
            Ok(FxRateTransaction {
                state: std::sync::Arc::clone(&self.transaction_state),
            })
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FxRateTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = self
                .state
                .lock()
                .map_err(|_| TransactionError::CommitFailed)?;
            state.commit_count += 1;
            if state.commit_error {
                Err(TransactionError::CommitFailed)
            } else {
                Ok(())
            }
        }
    }

    impl FxRateSnapshotRepositoryFactory<FxRateTransaction> for LatestFxRates {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FxRateTransaction,
        ) -> impl FxRateSnapshotRepository + 'tx {
            FxRateSnapshotRepositoryFake {
                response: self.response.clone(),
            }
        }
    }

    #[async_trait::async_trait]
    impl FxRateSnapshotRepository for FxRateSnapshotRepositoryFake {
        async fn find_latest(
            &mut self,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            match &self.response {
                LatestFxRateResponse::Found(snapshot) => Ok(Some(snapshot.clone())),
                LatestFxRateResponse::Missing => Ok(None),
                LatestFxRateResponse::Invalid => {
                    Err(FxRateSnapshotRepositoryError::InvalidPersistedSnapshot {
                        source: box_error(std::io::Error::other("invalid test FX snapshot")),
                    })
                }
                LatestFxRateResponse::ReadFailed => {
                    Err(FxRateSnapshotRepositoryError::ReadFailed {
                        source: box_error(std::io::Error::other("unavailable test FX snapshot")),
                    })
                }
            }
        }

        async fn find_latest_at_or_before(
            &mut self,
            _timestamp: time::OffsetDateTime,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(None)
        }

        async fn find_by_id(
            &mut self,
            _id: FxRateId,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(None)
        }

        async fn find_by_ids(
            &mut self,
            _ids: &[FxRateId],
        ) -> Result<Vec<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(Vec::new())
        }

        async fn insert(
            &mut self,
            _snapshot: &fxrate_core::NewFxRateSnapshot,
            _source_event_id: &str,
        ) -> Result<fxrate_service::ports::FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError>
        {
            Ok(fxrate_service::ports::FxRateSnapshotInsertOutcome::Duplicate)
        }
    }

    #[derive(Default)]
    struct Index {
        upserts: Mutex<Vec<(i64, FxRateId)>>,
        deletes: Mutex<Vec<i64>>,
    }

    #[async_trait::async_trait]
    impl SearchFilterIndex for Index {
        async fn upsert(
            &self,
            projection: &CompiledSearchFilterProjection,
        ) -> Result<SearchFilterProjectionWriteOutcome, SearchFilterIndexError> {
            self.upserts
                .lock()
                .map_err(|_| SearchFilterIndexError::WriteFailed {
                    source: box_error(std::io::Error::other("test mutex poisoned")),
                })?
                .push((
                    projection.projection.source_version,
                    projection.price_filter_plan.fx_rate_id,
                ));
            Ok(SearchFilterProjectionWriteOutcome::Applied)
        }

        async fn delete(
            &self,
            _id: UserSearchFilterId,
            source_version: i64,
        ) -> Result<SearchFilterProjectionWriteOutcome, SearchFilterIndexError> {
            self.deletes
                .lock()
                .map_err(|_| SearchFilterIndexError::DeleteFailed {
                    source: box_error(std::io::Error::other("test mutex poisoned")),
                })?
                .push(source_version);
            Ok(SearchFilterProjectionWriteOutcome::Applied)
        }

        async fn percolate(
            &self,
            _product: &product_service::ports::ProductSearchFilterMatchSource,
            _sale_snapshot: Option<&fxrate_core::FxRateSnapshot>,
        ) -> Result<Vec<SearchFilterView>, SearchFilterIndexError> {
            Ok(Vec::new())
        }

        async fn query(
            &self,
            _query: &SearchFilterIndexQuery,
        ) -> Result<CursoredResult<SearchFilterView, serde_json::Value>, SearchFilterIndexError>
        {
            Ok(CursoredResult::default())
        }
    }

    fn latest_fx_rates() -> Result<LatestFxRates, Box<dyn std::error::Error>> {
        let snapshot = NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            time::OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| FxRateQuote::new(currency, FX_RATE_SCALE)),
        )?
        .into_persisted(1_i64.try_into()?);
        Ok(LatestFxRates::new(LatestFxRateResponse::Found(snapshot)))
    }

    fn projection(version: i64) -> SearchFilterProjection {
        SearchFilterProjection {
            view: SearchFilterView {
                search_filter_id: UserSearchFilterId::new(),
                user_id: UserId::new(),
                name: UserSearchFilterName::from("daily"),
                notifications: true,
                state: ResourceState::Active,
                search: ProductSearch::new(Language::En, Currency::Eur),
                embedding: None,
                created: datetime!(2026-01-01 0:00 UTC),
                updated: datetime!(2026-01-01 0:00 UTC),
                last_hybrid_search_matched: datetime!(2026-01-01 0:00 UTC),
            },
            source_version: version,
        }
    }

    #[tokio::test]
    async fn should_project_current_authoritative_version_for_an_old_cdc_change()
    -> Result<(), Box<dyn std::error::Error>> {
        let current = projection(4);
        let id = current.view.search_filter_id;
        let source = Source {
            projection: Mutex::new(Some(current)),
        };
        let index = Index::default();
        let latest_fx_rates = latest_fx_rates()?;
        let expected_fx_rate_id = match &latest_fx_rates.response {
            LatestFxRateResponse::Found(snapshot) => snapshot.id(),
            _ => return Err(std::io::Error::other("test FX snapshot is missing").into()),
        };
        let handler = ProjectSearchFilterChangeHandler::new(
            latest_fx_rates.clone(),
            source,
            latest_fx_rates,
            index,
        );

        handler
            .execute(ProjectSearchFilterChangeCommand {
                search_filter_id: id,
                source_version: 2,
                operation: SearchFilterProjectionOperation::Upsert,
            })
            .await?;

        let upserts = handler
            .index
            .upserts
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?
            .clone();
        let deletes = handler
            .index
            .deletes
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?
            .clone();
        assert_eq!(vec![(4, expected_fx_rate_id)], upserts);
        assert!(deletes.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_write_successor_delete_tombstone_for_deleted_source()
    -> Result<(), Box<dyn std::error::Error>> {
        let index = Index::default();
        let latest_fx_rates = LatestFxRates::new(LatestFxRateResponse::Missing);
        let handler = ProjectSearchFilterChangeHandler::new(
            latest_fx_rates.clone(),
            Source::default(),
            latest_fx_rates,
            index,
        );

        handler
            .execute(ProjectSearchFilterChangeCommand {
                search_filter_id: UserSearchFilterId::new(),
                source_version: 7,
                operation: SearchFilterProjectionOperation::Delete,
            })
            .await?;

        let deletes = handler
            .index
            .deletes
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?
            .clone();
        assert_eq!(vec![8], deletes);
        assert_eq!(
            0,
            handler
                .unit_of_work
                .transaction_state
                .lock()
                .map_err(|_| std::io::Error::other("test mutex poisoned"))?
                .commit_count
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_delete_version_overflow() {
        let latest_fx_rates = LatestFxRates::new(LatestFxRateResponse::Missing);
        let handler = ProjectSearchFilterChangeHandler::new(
            latest_fx_rates.clone(),
            Source::default(),
            latest_fx_rates,
            Index::default(),
        );

        let result = handler
            .execute(ProjectSearchFilterChangeCommand {
                search_filter_id: UserSearchFilterId::new(),
                source_version: i64::MAX,
                operation: SearchFilterProjectionOperation::Delete,
            })
            .await;

        assert!(matches!(
            result,
            Err(ProjectSearchFilterChangeError::DeleteVersionOverflow)
        ));
    }

    #[tokio::test]
    async fn should_reject_non_positive_source_versions() {
        let latest_fx_rates = LatestFxRates::new(LatestFxRateResponse::Missing);
        let handler = ProjectSearchFilterChangeHandler::new(
            latest_fx_rates.clone(),
            Source::default(),
            latest_fx_rates,
            Index::default(),
        );

        let result = handler
            .execute(ProjectSearchFilterChangeCommand {
                search_filter_id: UserSearchFilterId::new(),
                source_version: 0,
                operation: SearchFilterProjectionOperation::Upsert,
            })
            .await;

        assert!(matches!(
            result,
            Err(ProjectSearchFilterChangeError::InvalidSourceVersion)
        ));
    }

    #[tokio::test]
    async fn should_return_typed_errors_for_missing_invalid_and_failed_fx_snapshot_reads()
    -> Result<(), Box<dyn std::error::Error>> {
        for (response, expected) in [
            (LatestFxRateResponse::Missing, "missing"),
            (LatestFxRateResponse::Invalid, "invalid"),
            (LatestFxRateResponse::ReadFailed, "read failed"),
        ] {
            let current = projection(1);
            let latest_fx_rates = LatestFxRates::new(response);
            let handler = ProjectSearchFilterChangeHandler::new(
                latest_fx_rates.clone(),
                Source {
                    projection: Mutex::new(Some(current.clone())),
                },
                latest_fx_rates,
                Index::default(),
            );

            let result = handler
                .execute(ProjectSearchFilterChangeCommand {
                    search_filter_id: current.view.search_filter_id,
                    source_version: 1,
                    operation: SearchFilterProjectionOperation::Upsert,
                })
                .await;

            match expected {
                "missing" => {
                    assert!(matches!(
                        result,
                        Err(ProjectSearchFilterChangeError::FxRateSnapshotMissing)
                    ));
                    assert_eq!(
                        1,
                        handler
                            .unit_of_work
                            .transaction_state
                            .lock()
                            .map_err(|_| std::io::Error::other("test mutex poisoned"))?
                            .commit_count
                    );
                }
                "invalid" => assert!(matches!(
                    result,
                    Err(ProjectSearchFilterChangeError::FxRateSnapshotInvalid { .. })
                )),
                "read failed" => assert!(matches!(
                    result,
                    Err(ProjectSearchFilterChangeError::FxRateSnapshotReadFailed { .. })
                )),
                _ => return Err(std::io::Error::other("unexpected test case").into()),
            }
            assert!(
                handler
                    .index
                    .upserts
                    .lock()
                    .map_err(|_| std::io::Error::other("test mutex poisoned"))?
                    .is_empty()
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_map_fx_snapshot_transaction_begin_and_commit_failures()
    -> Result<(), Box<dyn std::error::Error>> {
        let current = projection(1);
        let begin_fx_rates = latest_fx_rates()?;
        begin_fx_rates
            .transaction_state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?
            .begin_error = true;
        let begin_handler = ProjectSearchFilterChangeHandler::new(
            begin_fx_rates.clone(),
            Source {
                projection: Mutex::new(Some(current.clone())),
            },
            begin_fx_rates,
            Index::default(),
        );

        let begin_result = begin_handler
            .execute(ProjectSearchFilterChangeCommand {
                search_filter_id: current.view.search_filter_id,
                source_version: 1,
                operation: SearchFilterProjectionOperation::Upsert,
            })
            .await;
        assert!(matches!(
            begin_result,
            Err(ProjectSearchFilterChangeError::BeginFxRateSnapshotTransactionFailed { .. })
        ));

        let commit_fx_rates = latest_fx_rates()?;
        commit_fx_rates
            .transaction_state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?
            .commit_error = true;
        let commit_handler = ProjectSearchFilterChangeHandler::new(
            commit_fx_rates.clone(),
            Source {
                projection: Mutex::new(Some(current.clone())),
            },
            commit_fx_rates,
            Index::default(),
        );

        let commit_result = commit_handler
            .execute(ProjectSearchFilterChangeCommand {
                search_filter_id: current.view.search_filter_id,
                source_version: 1,
                operation: SearchFilterProjectionOperation::Upsert,
            })
            .await;
        assert!(matches!(
            commit_result,
            Err(ProjectSearchFilterChangeError::CommitFxRateSnapshotTransactionFailed { .. })
        ));
        Ok(())
    }
}
