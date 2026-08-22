use crate::ports::{
    ExistingSearchFilterMatchReader, PeriodicSearchFilterCandidate,
    PeriodicSearchFilterCandidateReadError, PeriodicSearchFilterCandidateReader,
    PeriodicSearchFilterMatchingRunLock, PeriodicSearchFilterMatchingRunLockError,
    PeriodicSearchFilterProgress, PeriodicSearchFilterProgressError,
    PeriodicSearchFilterProgressFactory, PeriodicSearchFilterProgressWriteOutcome,
    SearchFilterMatchWriteError, SearchFilterMatchWriter, SearchFilterMatchWriterFactory,
};
use crate::product_match_evaluator::{
    ProductMatchEvaluationOutcome, ProductMatchEvaluationRequest, evaluate_product_matches,
};
use application::error::{BoxError, box_error};
use application::pagination::Cursor;
use application::transaction::{Transaction, UnitOfWork};

use domain_primitives::query::range_query::RangeQuery;
use fxrate_core::{FxRateSnapshot, FxRateSnapshotError};
use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use large_language_model::LargeLanguageModel;
use product_core::product::ProductPriceValuationBasis;

use product_core::product_search::ProductSearch;
use product_service::ports::{
    CompiledProductSearch, ProductCurrentRevisionCheck, ProductCurrentRevisionGuard,
    ProductCurrentRevisionGuardFactory, ProductCurrentRevisionRef, ProductPriceFilterPlan,
    ProductSearchFilterMatchSource, ProductSearchFilterMatchSourceReadError,
    ProductSearchFilterMatchSourceReader, ProductSearchFilterMatchSourceReaderFactory,
    ProductSearchFilterMatchSourceRef, ProductSearchReadError, ProductSearchReadRequest,
    ProductSearchReader,
};

use search_filter_core::{PriceMatchValuation, SearchFilterProductMatch};
use std::collections::HashMap;
use std::num::NonZeroUsize;
use time::{Duration, OffsetDateTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeriodicSearchFilterMatchingPolicy {
    pub filter_page_size: NonZeroUsize,
    pub hybrid_scan_limit: NonZeroUsize,
    pub evaluation_limit: NonZeroUsize,
    pub llm_concurrency: NonZeroUsize,
    pub max_attempts: NonZeroUsize,
    pub projection_lag: Duration,
    pub replay_overlap: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunPeriodicSearchFilterMatchingCommand {
    pub started_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunPeriodicSearchFilterMatchingOutcome {
    Applied(PeriodicSearchFilterMatchingReport),
    SkippedAlreadyRunning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeriodicSearchFilterMatchingReport {
    pub window_end: OffsetDateTime,
    pub filters_selected: usize,
    pub filters_completed: usize,
    pub filters_changed_or_inactive: usize,
    pub filters_failed: usize,
    pub candidates_scanned: usize,
    pub candidates_existing: usize,
    pub candidates_missing_source: usize,
    pub candidates_stale: usize,
    pub candidates_rejected: usize,
    pub permanent_evaluation_failures: usize,
    pub retryable_evaluation_failures: usize,
    pub matches_inserted: usize,
    pub matches_duplicate: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum RunPeriodicSearchFilterMatchingError {
    #[error("periodic matching policy is invalid")]
    InvalidPolicy,
    #[error("failed to acquire periodic search-filter matching run lock")]
    RunLockFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to release periodic search-filter matching run lock")]
    RunLockReleaseFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin FX snapshot transaction")]
    BeginFxSnapshotTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("FX snapshot read failed")]
    FxSnapshotReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("FX snapshot is invalid")]
    FxSnapshotInvalid {
        #[source]
        source: BoxError,
    },
    #[error("no FX snapshot exists at the periodic matching window end")]
    FxSnapshotNotFound,
    #[error("failed to commit FX snapshot transaction")]
    CommitFxSnapshotTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("periodic search-filter candidate read failed")]
    CandidateReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("periodic search-filter candidate persisted state is invalid")]
    CandidateStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("Product search failed")]
    ProductSearchFailed,
    #[error("Product search result is invalid")]
    ProductSearchResultInvalid,
    #[error("existing search-filter match read failed")]
    ExistingMatchReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin Product source transaction")]
    BeginProductSourceTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("Product source read failed")]
    ProductSourceReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("Product source persisted state is invalid")]
    ProductSourceStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("Product source identity does not match its requested reference")]
    ProductSourceMismatch,
    #[error("failed to commit Product source transaction")]
    CommitProductSourceTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin final periodic match transaction")]
    BeginFinalTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("periodic matching progress operation failed")]
    ProgressFailed {
        #[source]
        source: BoxError,
    },
    #[error("Product revision check failed")]
    ProductRevisionCheckFailed {
        #[source]
        source: BoxError,
    },
    #[error("search-filter match persistence failed")]
    MatchPersistenceFailed {
        #[source]
        source: BoxError,
    },
    #[error("search-filter match persisted state is invalid")]
    MatchStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("failed to commit final periodic match transaction")]
    CommitFinalTransactionFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait RunPeriodicSearchFilterMatchingUseCase: Send + Sync {
    async fn execute(
        &self,
        command: RunPeriodicSearchFilterMatchingCommand,
    ) -> Result<RunPeriodicSearchFilterMatchingOutcome, RunPeriodicSearchFilterMatchingError>;
}

pub struct RunPeriodicSearchFilterMatchingHandler<U, L, C, F, P, X, S, E, G, W, Q> {
    unit_of_work: U,
    run_lock: L,
    candidates: C,
    fx_rates: F,
    products: P,
    existing_matches: X,
    sources: S,
    evaluator: E,
    revisions: G,
    matches: W,
    progress: Q,
    policy: PeriodicSearchFilterMatchingPolicy,
}

impl<U, L, C, F, P, X, S, E, G, W, Q>
    RunPeriodicSearchFilterMatchingHandler<U, L, C, F, P, X, S, E, G, W, Q>
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        unit_of_work: U,
        run_lock: L,
        candidates: C,
        fx_rates: F,
        products: P,
        existing_matches: X,
        sources: S,
        evaluator: E,
        revisions: G,
        matches: W,
        progress: Q,
        policy: PeriodicSearchFilterMatchingPolicy,
    ) -> Result<Self, RunPeriodicSearchFilterMatchingError> {
        if policy.evaluation_limit > policy.hybrid_scan_limit
            || policy.projection_lag.is_negative()
            || policy.replay_overlap.is_negative()
        {
            return Err(RunPeriodicSearchFilterMatchingError::InvalidPolicy);
        }
        Ok(Self {
            unit_of_work,
            run_lock,
            candidates,
            fx_rates,
            products,
            existing_matches,
            sources,
            evaluator,
            revisions,
            matches,
            progress,
            policy,
        })
    }
}

#[async_trait::async_trait]
impl<U, L, C, F, P, X, S, E, G, W, Q> RunPeriodicSearchFilterMatchingUseCase
    for RunPeriodicSearchFilterMatchingHandler<U, L, C, F, P, X, S, E, G, W, Q>
where
    U: UnitOfWork,
    L: PeriodicSearchFilterMatchingRunLock,
    C: PeriodicSearchFilterCandidateReader,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
    P: ProductSearchReader,
    X: ExistingSearchFilterMatchReader,
    S: ProductSearchFilterMatchSourceReaderFactory<U::Tx>,
    E: LargeLanguageModel,
    G: ProductCurrentRevisionGuardFactory<U::Tx>,
    W: SearchFilterMatchWriterFactory<U::Tx>,
    Q: PeriodicSearchFilterProgressFactory<U::Tx>,
{
    #[tracing::instrument(name = "run_periodic_search_filter_matching", skip_all, fields(window_end = tracing::field::Empty))]
    async fn execute(
        &self,
        command: RunPeriodicSearchFilterMatchingCommand,
    ) -> Result<RunPeriodicSearchFilterMatchingOutcome, RunPeriodicSearchFilterMatchingError> {
        let Some(lease) = self.run_lock.try_acquire().await.map_err(run_lock_error)? else {
            return Ok(RunPeriodicSearchFilterMatchingOutcome::SkippedAlreadyRunning);
        };
        let result = self.execute_locked(command).await;
        let release_result = lease.release().await.map_err(run_lock_error);
        match (result, release_result) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), _) => Err(error),
            (_, Err(error)) => Err(error),
        }
    }
}

impl<U, L, C, F, P, X, S, E, G, W, Q>
    RunPeriodicSearchFilterMatchingHandler<U, L, C, F, P, X, S, E, G, W, Q>
where
    U: UnitOfWork,
    C: PeriodicSearchFilterCandidateReader,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
    P: ProductSearchReader,
    X: ExistingSearchFilterMatchReader,
    S: ProductSearchFilterMatchSourceReaderFactory<U::Tx>,
    E: LargeLanguageModel,
    G: ProductCurrentRevisionGuardFactory<U::Tx>,
    W: SearchFilterMatchWriterFactory<U::Tx>,
    Q: PeriodicSearchFilterProgressFactory<U::Tx>,
{
    async fn execute_locked(
        &self,
        command: RunPeriodicSearchFilterMatchingCommand,
    ) -> Result<RunPeriodicSearchFilterMatchingOutcome, RunPeriodicSearchFilterMatchingError> {
        let window_end = command.started_at - self.policy.projection_lag;
        tracing::Span::current().record("window_end", tracing::field::display(window_end));
        let snapshot = load_snapshot(&self.unit_of_work, &self.fx_rates, window_end).await?;
        let mut report = PeriodicSearchFilterMatchingReport {
            window_end,
            filters_selected: 0,
            filters_completed: 0,
            filters_changed_or_inactive: 0,
            filters_failed: 0,
            candidates_scanned: 0,
            candidates_existing: 0,
            candidates_missing_source: 0,
            candidates_stale: 0,
            candidates_rejected: 0,
            permanent_evaluation_failures: 0,
            retryable_evaluation_failures: 0,
            matches_inserted: 0,
            matches_duplicate: 0,
        };
        let mut after = None;
        loop {
            let page = self
                .candidates
                .find_active_page(
                    after,
                    self.policy.filter_page_size.get(),
                    command.started_at,
                )
                .await
                .map_err(candidate_read_error)?;
            if page.is_empty() {
                break;
            }
            after = page.last().map(|candidate| candidate.search_filter_id);
            for candidate in page {
                report.filters_selected += 1;
                let mut completed = false;
                for _ in 0..self.policy.max_attempts.get() {
                    match self
                        .process_filter(&candidate, &snapshot, window_end, &mut report)
                        .await?
                    {
                        FilterOutcome::Completed => {
                            completed = true;
                            break;
                        }
                        FilterOutcome::ChangedOrInactive => {
                            report.filters_changed_or_inactive += 1;
                            completed = true;
                            break;
                        }
                        FilterOutcome::Retryable => continue,
                    }
                }
                if completed {
                    report.filters_completed += 1;
                } else {
                    report.filters_failed += 1;
                }
            }
        }
        Ok(RunPeriodicSearchFilterMatchingOutcome::Applied(report))
    }

    async fn process_filter(
        &self,
        filter: &PeriodicSearchFilterCandidate,
        snapshot: &FxRateSnapshot,
        window_end: OffsetDateTime,
        report: &mut PeriodicSearchFilterMatchingReport,
    ) -> Result<FilterOutcome, RunPeriodicSearchFilterMatchingError> {
        let Some(embedding) = filter.embedding.as_deref() else {
            return Ok(FilterOutcome::Retryable);
        };
        let Some(search) = periodic_search(filter, window_end, self.policy.replay_overlap) else {
            return self
                .commit_filter(filter, window_end, Vec::new(), true, report)
                .await;
        };
        let price_filter_plan =
            ProductPriceFilterPlan::compile(snapshot.clone(), search.currency, search.price_query)
                .map_err(|source: FxRateSnapshotError| {
                    RunPeriodicSearchFilterMatchingError::FxSnapshotInvalid {
                        source: box_error(source),
                    }
                })?;
        let request = ProductSearchReadRequest {
            compiled_search: CompiledProductSearch {
                search,
                price_filter_plan: price_filter_plan.clone(),
            },
            sort: None,
            cursor: Some(Cursor {
                size: self.policy.hybrid_scan_limit.get() as u64,
                search_after: None,
            }),
        };
        let result = self
            .products
            .search_hybrid(&request, embedding)
            .await
            .map_err(product_search_error)?;
        report.candidates_scanned += result.items.len();
        let ids = result
            .items
            .iter()
            .map(|item| item.product_id)
            .collect::<Vec<_>>();
        let existing = self
            .existing_matches
            .find_existing_product_ids(filter.search_filter_id, &ids)
            .await
            .map_err(
                |source| RunPeriodicSearchFilterMatchingError::ExistingMatchReadFailed {
                    source: box_error(source),
                },
            )?;
        report.candidates_existing += existing.len();
        let refs = result
            .items
            .into_iter()
            .filter(|item| !existing.contains(&item.product_id))
            .take(self.policy.evaluation_limit.get())
            .map(|item| ProductSearchFilterMatchSourceRef {
                product_id: item.product_id,
                event_id: item.event_id,
            })
            .collect::<Vec<_>>();
        let sources = self.load_sources(&refs).await?;
        let mut evaluations = Vec::new();
        for reference in refs {
            match sources.get(&reference) {
                None => report.candidates_missing_source += 1,
                Some(source)
                    if source.product_id != reference.product_id
                        || source.event_id != reference.event_id =>
                {
                    return Err(RunPeriodicSearchFilterMatchingError::ProductSourceMismatch);
                }
                Some(source) if source.current_event_id != reference.event_id => {
                    report.candidates_stale += 1
                }
                Some(source) => evaluations.push(ProductMatchEvaluationRequest {
                    key: reference,
                    product: source,
                    search_description: filter
                        .search
                        .enhanced_search_description
                        .as_deref()
                        .unwrap_or_default(),
                    search_language: filter.search.language,
                }),
            }
        }
        let evaluations =
            evaluate_product_matches(&self.evaluator, evaluations, self.policy.llm_concurrency)
                .await;
        let mut retryable = false;
        let mut accepted = Vec::new();
        for evaluation in evaluations {
            match evaluation.outcome {
                ProductMatchEvaluationOutcome::Matched(reason) => {
                    let source = sources
                        .get(&evaluation.key)
                        .ok_or(RunPeriodicSearchFilterMatchingError::ProductSourceMismatch)?;
                    let valuation =
                        filter
                            .search
                            .price_query
                            .as_ref()
                            .map(|_| PriceMatchValuation {
                                basis: if source.sale_valuation.is_some() {
                                    ProductPriceValuationBasis::Sale
                                } else {
                                    ProductPriceValuationBasis::Current
                                },
                                fx_rate_id: if let Some(sale) = source.sale_valuation {
                                    sale.fx_rate_id
                                } else {
                                    price_filter_plan.fx_rate_id
                                },
                            });
                    accepted.push(SearchFilterProductMatch {
                        user_id: filter.user_id,
                        user_search_filter_id: filter.search_filter_id,
                        user_search_filter_name: Some(filter.name.clone()),
                        product_id: source.product_id,
                        origin_event_id: source.event_id,
                        price_match_valuation: valuation,
                        enhanced_match_reason: Some(reason),
                        feedback: None,
                    });
                }
                ProductMatchEvaluationOutcome::Rejected => report.candidates_rejected += 1,
                ProductMatchEvaluationOutcome::PermanentFailure(_) => {
                    report.permanent_evaluation_failures += 1
                }
                ProductMatchEvaluationOutcome::RetryableFailure(_) => {
                    report.retryable_evaluation_failures += 1;
                    retryable = true;
                }
            }
        }
        let outcome = self
            .commit_filter(filter, window_end, accepted, !retryable, report)
            .await?;
        Ok(
            if matches!(outcome, FilterOutcome::Completed) && retryable {
                FilterOutcome::Retryable
            } else {
                outcome
            },
        )
    }

    async fn load_sources(
        &self,
        refs: &[ProductSearchFilterMatchSourceRef],
    ) -> Result<
        HashMap<ProductSearchFilterMatchSourceRef, ProductSearchFilterMatchSource>,
        RunPeriodicSearchFilterMatchingError,
    > {
        if refs.is_empty() {
            return Ok(HashMap::new());
        }
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            RunPeriodicSearchFilterMatchingError::BeginProductSourceTransactionFailed {
                source: box_error(source),
            }
        })?;
        let sources = self
            .sources
            .in_transaction(&mut tx)
            .find_sources(refs)
            .await
            .map_err(product_source_error)?;
        tx.commit().await.map_err(|source| {
            RunPeriodicSearchFilterMatchingError::CommitProductSourceTransactionFailed {
                source: box_error(source),
            }
        })?;
        Ok(sources)
    }

    async fn commit_filter(
        &self,
        filter: &PeriodicSearchFilterCandidate,
        window_end: OffsetDateTime,
        matches: Vec<SearchFilterProductMatch>,
        advance_progress: bool,
        report: &mut PeriodicSearchFilterMatchingReport,
    ) -> Result<FilterOutcome, RunPeriodicSearchFilterMatchingError> {
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            RunPeriodicSearchFilterMatchingError::BeginFinalTransactionFailed {
                source: box_error(source),
            }
        })?;
        let matched_through = self
            .progress
            .in_transaction(&mut tx)
            .lock_and_read(filter.search_filter_id, filter.created)
            .await
            .map_err(progress_error)?;
        if matched_through > filter.matched_through {
            return Ok(FilterOutcome::ChangedOrInactive);
        }
        let refs = matches
            .iter()
            .map(|item| ProductCurrentRevisionRef {
                product_id: item.product_id,
                expected_event_id: item.origin_event_id,
            })
            .collect::<Vec<_>>();
        let checks = self
            .revisions
            .in_transaction(&mut tx)
            .lock_and_check_all(&refs)
            .await
            .map_err(
                |source| RunPeriodicSearchFilterMatchingError::ProductRevisionCheckFailed {
                    source: box_error(source),
                },
            )?;
        let matches = matches
            .into_iter()
            .filter(|item| {
                checks.get(&ProductCurrentRevisionRef {
                    product_id: item.product_id,
                    expected_event_id: item.origin_event_id,
                }) == Some(&ProductCurrentRevisionCheck::Current)
            })
            .collect::<Vec<_>>();
        let persisted = self
            .matches
            .in_transaction(&mut tx)
            .insert_all_if_absent(&matches)
            .await
            .map_err(match_write_error)?;
        if advance_progress {
            let progress = self
                .progress
                .in_transaction(&mut tx)
                .compare_and_set(filter.search_filter_id, matched_through, window_end)
                .await
                .map_err(progress_error)?;
            if progress == PeriodicSearchFilterProgressWriteOutcome::Superseded {
                return Ok(FilterOutcome::ChangedOrInactive);
            }
        }
        tx.commit().await.map_err(|source| {
            RunPeriodicSearchFilterMatchingError::CommitFinalTransactionFailed {
                source: box_error(source),
            }
        })?;
        report.matches_inserted += persisted.inserted;
        report.matches_duplicate += persisted.already_exists;
        Ok(FilterOutcome::Completed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilterOutcome {
    Completed,
    ChangedOrInactive,
    Retryable,
}

async fn load_snapshot<U, F>(
    unit_of_work: &U,
    fx_rates: &F,
    window_end: OffsetDateTime,
) -> Result<FxRateSnapshot, RunPeriodicSearchFilterMatchingError>
where
    U: UnitOfWork,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
{
    let mut tx = unit_of_work.begin().await.map_err(|source| {
        RunPeriodicSearchFilterMatchingError::BeginFxSnapshotTransactionFailed {
            source: box_error(source),
        }
    })?;
    let snapshot = fx_rates
        .in_transaction(&mut tx)
        .find_latest_at_or_before(window_end)
        .await
        .map_err(fx_snapshot_error)?;
    tx.commit().await.map_err(|source| {
        RunPeriodicSearchFilterMatchingError::CommitFxSnapshotTransactionFailed {
            source: box_error(source),
        }
    })?;
    snapshot.ok_or(RunPeriodicSearchFilterMatchingError::FxSnapshotNotFound)
}

fn periodic_search(
    filter: &PeriodicSearchFilterCandidate,
    window_end: OffsetDateTime,
    replay_overlap: Duration,
) -> Option<ProductSearch> {
    let replay_start = filter.matched_through - replay_overlap;
    let min = filter
        .search
        .updated_query
        .as_ref()
        .and_then(|range| range.min)
        .map_or(replay_start, |value| value.max(replay_start));
    let max = filter
        .search
        .updated_query
        .as_ref()
        .and_then(|range| range.max)
        .map_or(window_end, |value| value.min(window_end));
    if min > max {
        return None;
    }
    let mut search = filter.search.clone();
    search.updated_query = Some(RangeQuery {
        min: Some(min),
        max: Some(max),
    });
    if let Some(query) = search
        .enhanced_search_description
        .as_ref()
        .filter(|description| !description.trim().is_empty())
        .and_then(|description| description.as_ref().try_into().ok())
        .filter(|query| {
            !search
                .product_query
                .iter()
                .any(|existing| existing == query)
        })
    {
        search.product_query.push(query)
    }
    Some(search)
}

fn run_lock_error(
    error: PeriodicSearchFilterMatchingRunLockError,
) -> RunPeriodicSearchFilterMatchingError {
    match error {
        PeriodicSearchFilterMatchingRunLockError::LockFailed { source } => {
            RunPeriodicSearchFilterMatchingError::RunLockFailed { source }
        }
        PeriodicSearchFilterMatchingRunLockError::ReleaseFailed { source } => {
            RunPeriodicSearchFilterMatchingError::RunLockReleaseFailed { source }
        }
    }
}
fn candidate_read_error(
    error: PeriodicSearchFilterCandidateReadError,
) -> RunPeriodicSearchFilterMatchingError {
    match error {
        PeriodicSearchFilterCandidateReadError::InvalidPersistedState { source } => {
            RunPeriodicSearchFilterMatchingError::CandidateStateInvalid { source }
        }
        PeriodicSearchFilterCandidateReadError::ReadFailed { source } => {
            RunPeriodicSearchFilterMatchingError::CandidateReadFailed { source }
        }
    }
}
fn fx_snapshot_error(error: FxRateSnapshotRepositoryError) -> RunPeriodicSearchFilterMatchingError {
    match error {
        FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { source } => {
            RunPeriodicSearchFilterMatchingError::FxSnapshotInvalid { source }
        }
        error => RunPeriodicSearchFilterMatchingError::FxSnapshotReadFailed {
            source: box_error(error),
        },
    }
}
fn product_search_error(error: ProductSearchReadError) -> RunPeriodicSearchFilterMatchingError {
    match error {
        ProductSearchReadError::ProductSearchQueryFailed => {
            RunPeriodicSearchFilterMatchingError::ProductSearchFailed
        }
        ProductSearchReadError::ProductSearchReadModelInvalid => {
            RunPeriodicSearchFilterMatchingError::ProductSearchResultInvalid
        }
    }
}
fn product_source_error(
    error: ProductSearchFilterMatchSourceReadError,
) -> RunPeriodicSearchFilterMatchingError {
    match error {
        ProductSearchFilterMatchSourceReadError::InvalidPersistedState { source } => {
            RunPeriodicSearchFilterMatchingError::ProductSourceStateInvalid { source }
        }
        error => RunPeriodicSearchFilterMatchingError::ProductSourceReadFailed {
            source: box_error(error),
        },
    }
}
fn progress_error(
    error: PeriodicSearchFilterProgressError,
) -> RunPeriodicSearchFilterMatchingError {
    RunPeriodicSearchFilterMatchingError::ProgressFailed {
        source: box_error(error),
    }
}
fn match_write_error(error: SearchFilterMatchWriteError) -> RunPeriodicSearchFilterMatchingError {
    match error {
        SearchFilterMatchWriteError::InvalidPersistedState { source } => {
            RunPeriodicSearchFilterMatchingError::MatchStateInvalid { source }
        }
        error => RunPeriodicSearchFilterMatchingError::MatchPersistenceFailed {
            source: box_error(error),
        },
    }
}
