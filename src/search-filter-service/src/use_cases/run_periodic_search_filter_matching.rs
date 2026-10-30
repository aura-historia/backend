use crate::ports::{
    ExistingSearchFilterMatchReader, PeriodicSearchFilterCandidate,
    PeriodicSearchFilterCandidatePageRequest, PeriodicSearchFilterCandidateReadError,
    PeriodicSearchFilterCandidateReader, PeriodicSearchFilterMatchingRunLock,
    PeriodicSearchFilterMatchingRunLockError, PeriodicSearchFilterProgress,
    PeriodicSearchFilterProgressError, PeriodicSearchFilterProgressFactory,
    PeriodicSearchFilterProgressLockOutcome, PeriodicSearchFilterProgressWriteOutcome,
    SearchFilterMatchWriteError, SearchFilterMatchWriter, SearchFilterMatchWriterFactory,
};
use crate::product_match_evaluator::{
    ProductListingMatchEvaluationOutcome, ProductListingMatchEvaluationRequest,
    evaluate_product_matches,
};
use application::error::{BoxError, box_error};
use application::pagination::Cursor;
use application::transaction::{Transaction, UnitOfWork};
use classifier_model::{ClassifierModel, Probability};

use domain_primitives::query::range_query::RangeQuery;
use fxrate_core::{FxRateSnapshot, FxRateSnapshotError};
use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use product_listing_core::{
    listing_availability::ListingAvailability, listing_lifecycle::ListingLifecycle,
    product_listing::ProductListingPriceValuationBasis,
};

use notification_service::saved_search_digest::{
    SavedSearchDigestStore, SavedSearchDigestStoreFactory,
};
use product_listing_core::product_listing_search::ProductListingSearch;
use product_listing_service::ports::{
    CompiledProductListingSearch, ProductListingCurrentEventCheck, ProductListingCurrentEventGuard,
    ProductListingCurrentEventGuardFactory, ProductListingCurrentEventRef,
    ProductListingPriceFilterPlan, ProductListingSearchFilterMatchSource,
    ProductListingSearchFilterMatchSourceReadError, ProductListingSearchFilterMatchSourceReader,
    ProductListingSearchFilterMatchSourceReaderFactory, ProductListingSearchFilterMatchSourceRef,
    ProductListingSearchReadError, ProductListingSearchReadRequest, ProductListingSearchReader,
};

use search_filter_core::{PriceMatchValuation, SearchFilterProductListingMatch};
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use time::{Duration, OffsetDateTime};
use tracing::Instrument;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeriodicSearchFilterMatchingPolicy {
    pub filter_page_size: NonZeroUsize,
    pub hybrid_scan_limit: NonZeroUsize,
    pub evaluation_limit: NonZeroUsize,
    pub classification_concurrency: NonZeroUsize,
    /// One preserves singleton evaluation; larger values enable budgeted text batches.
    pub classification_batch_candidates: NonZeroUsize,
    pub should_show_threshold: Probability,
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
    Applied(Box<PeriodicSearchFilterMatchingReport>),
    SkippedAlreadyRunning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeriodicSearchFilterMatchingReport {
    pub window_end: OffsetDateTime,
    pub filters_selected: usize,
    pub filters_completed: usize,
    pub filters_already_covered: usize,
    pub filters_changed_or_inactive: usize,
    pub filters_progress_superseded: usize,
    pub filter_attempts: usize,
    pub filters_retried: usize,
    pub filters_failed: usize,
    pub filters_invalid_persisted_state: usize,
    pub candidates_scanned: usize,
    pub filters_scan_cap: usize,
    pub filters_candidate_cap: usize,
    pub candidates_accepted: usize,
    /// Submitted candidate requests, including retries and preparation failures.
    /// Actual provider attempts are the CLASSIFIER_INVOCATION event stream.
    pub classification_requests: usize,
    pub candidates_selected: usize,
    pub candidates_duplicate: usize,
    pub candidates_existing: usize,
    pub candidates_missing_source: usize,
    pub candidates_stale: usize,
    pub candidates_withdrawn: usize,
    pub candidates_rejected: usize,
    pub permanent_evaluation_failures: usize,
    pub retryable_evaluation_failures: usize,
    pub matches_inserted: usize,
    pub matches_duplicate: usize,
}

#[derive(Debug, Default)]
struct FilterAttemptReport {
    candidates_scanned: usize,
    filters_scan_cap: usize,
    filters_candidate_cap: usize,
    candidates_accepted: usize,
    classification_requests: usize,
    candidates_selected: usize,
    candidates_duplicate: usize,
    candidates_existing: usize,
    candidates_missing_source: usize,
    candidates_stale: usize,
    candidates_withdrawn: usize,
    candidates_rejected: usize,
    permanent_evaluation_failures: usize,
    retryable_evaluation_failures: usize,
    matches_inserted: usize,
    matches_duplicate: usize,
}

impl PeriodicSearchFilterMatchingReport {
    fn merge_attempt(&mut self, attempt: &FilterAttemptReport) {
        self.candidates_scanned += attempt.candidates_scanned;
        self.filters_scan_cap += attempt.filters_scan_cap;
        self.filters_candidate_cap += attempt.filters_candidate_cap;
        self.candidates_accepted += attempt.candidates_accepted;
        self.classification_requests += attempt.classification_requests;
        self.candidates_selected += attempt.candidates_selected;
        self.candidates_duplicate += attempt.candidates_duplicate;
        self.candidates_existing += attempt.candidates_existing;
        self.candidates_missing_source += attempt.candidates_missing_source;
        self.candidates_stale += attempt.candidates_stale;
        self.candidates_withdrawn += attempt.candidates_withdrawn;
        self.candidates_rejected += attempt.candidates_rejected;
        self.permanent_evaluation_failures += attempt.permanent_evaluation_failures;
        self.retryable_evaluation_failures += attempt.retryable_evaluation_failures;
        self.merge_durable_match_counts(attempt);
    }

    fn merge_durable_match_counts(&mut self, attempt: &FilterAttemptReport) {
        self.matches_inserted += attempt.matches_inserted;
        self.matches_duplicate += attempt.matches_duplicate;
    }
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
    #[error("ProductListing search failed")]
    ProductListingSearchFailed {
        #[source]
        source: BoxError,
    },
    #[error("ProductListing search result is invalid")]
    ProductListingSearchResultInvalid {
        #[source]
        source: BoxError,
    },
    #[error("existing search-filter match read failed")]
    ExistingMatchReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin ProductListing source transaction")]
    BeginProductListingSourceTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("ProductListing source read failed")]
    ProductListingSourceReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("ProductListing source persisted state is invalid")]
    ProductListingSourceStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("ProductListing source identity does not match its requested reference")]
    ProductListingSourceMismatch,
    #[error("failed to commit ProductListing source transaction")]
    CommitProductListingSourceTransactionFailed {
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
    #[error("ProductListing current event check failed")]
    ProductListingCurrentEventCheckFailed {
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
    #[error("saved-search digest finalization failed")]
    DigestFinalizationFailed {
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

pub struct RunPeriodicSearchFilterMatchingHandler<U, L, C, F, P, X, S, E, G, W, Q, D> {
    unit_of_work: U,
    run_lock: L,
    candidates: C,
    fx_rates: F,
    product_listings: P,
    existing_matches: X,
    sources: S,
    evaluator: E,
    current_event_guard: G,
    matches: W,
    progress: Q,
    digests: D,
    policy: PeriodicSearchFilterMatchingPolicy,
}

impl<U, L, C, F, P, X, S, E, G, W, Q, D>
    RunPeriodicSearchFilterMatchingHandler<U, L, C, F, P, X, S, E, G, W, Q, D>
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        unit_of_work: U,
        run_lock: L,
        candidates: C,
        fx_rates: F,
        product_listings: P,
        existing_matches: X,
        sources: S,
        evaluator: E,
        current_event_guard: G,
        matches: W,
        progress: Q,
        digests: D,
        policy: PeriodicSearchFilterMatchingPolicy,
    ) -> Result<Self, RunPeriodicSearchFilterMatchingError> {
        if policy.classification_batch_candidates.get() > 50
            || policy.evaluation_limit.get() > 50
            || policy.hybrid_scan_limit.get() > 100
            || policy.evaluation_limit > policy.hybrid_scan_limit
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
            product_listings,
            existing_matches,
            sources,
            evaluator,
            current_event_guard,
            matches,
            progress,
            digests,
            policy,
        })
    }
}

#[async_trait::async_trait]
impl<U, L, C, F, P, X, S, E, G, W, Q, D> RunPeriodicSearchFilterMatchingUseCase
    for RunPeriodicSearchFilterMatchingHandler<U, L, C, F, P, X, S, E, G, W, Q, D>
where
    U: UnitOfWork,
    L: PeriodicSearchFilterMatchingRunLock,
    C: PeriodicSearchFilterCandidateReader,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
    P: ProductListingSearchReader,
    X: ExistingSearchFilterMatchReader,
    S: ProductListingSearchFilterMatchSourceReaderFactory<U::Tx>,
    E: ClassifierModel,
    G: ProductListingCurrentEventGuardFactory<U::Tx>,
    W: SearchFilterMatchWriterFactory<U::Tx>,
    Q: PeriodicSearchFilterProgressFactory<U::Tx>,
    D: SavedSearchDigestStoreFactory<U::Tx>,
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
        if let Err(error) = &result {
            tracing::error!(error = %error, "search_filter.periodic_match.failed");
        }
        let release_result = lease.release().await.map_err(run_lock_error);
        match (result, release_result) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(release_error)) => {
                tracing::error!(error = %release_error, "search_filter.periodic_match.run_lock_release_failed");
                Err(error)
            }
            (Ok(_), Err(error)) => Err(error),
        }
    }
}

impl<U, L, C, F, P, X, S, E, G, W, Q, D>
    RunPeriodicSearchFilterMatchingHandler<U, L, C, F, P, X, S, E, G, W, Q, D>
where
    U: UnitOfWork,
    C: PeriodicSearchFilterCandidateReader,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
    P: ProductListingSearchReader,
    X: ExistingSearchFilterMatchReader,
    S: ProductListingSearchFilterMatchSourceReaderFactory<U::Tx>,
    E: ClassifierModel,
    G: ProductListingCurrentEventGuardFactory<U::Tx>,
    W: SearchFilterMatchWriterFactory<U::Tx>,
    Q: PeriodicSearchFilterProgressFactory<U::Tx>,
    D: SavedSearchDigestStoreFactory<U::Tx>,
{
    async fn execute_locked(
        &self,
        command: RunPeriodicSearchFilterMatchingCommand,
    ) -> Result<RunPeriodicSearchFilterMatchingOutcome, RunPeriodicSearchFilterMatchingError> {
        let mut run_tx = self.unit_of_work.begin().await.map_err(|source| {
            RunPeriodicSearchFilterMatchingError::BeginFinalTransactionFailed {
                source: box_error(source),
            }
        })?;
        let window_end = self
            .progress
            .in_transaction(&mut run_tx)
            .begin_run(command.started_at - self.policy.projection_lag)
            .await
            .map_err(progress_error)?;
        run_tx.commit().await.map_err(|source| {
            RunPeriodicSearchFilterMatchingError::CommitFinalTransactionFailed {
                source: box_error(source),
            }
        })?;
        tracing::Span::current().record("window_end", tracing::field::display(window_end));
        tracing::info!(window_end = %window_end, "search_filter.periodic_match.started");
        let snapshot = load_snapshot(&self.unit_of_work, &self.fx_rates, window_end).await?;
        let mut report = PeriodicSearchFilterMatchingReport {
            window_end,
            filters_selected: 0,
            filters_completed: 0,
            filters_already_covered: 0,
            filters_changed_or_inactive: 0,
            filters_progress_superseded: 0,
            filter_attempts: 0,
            filters_retried: 0,
            filters_failed: 0,
            filters_invalid_persisted_state: 0,
            candidates_scanned: 0,
            filters_scan_cap: 0,
            filters_candidate_cap: 0,
            candidates_accepted: 0,
            classification_requests: 0,
            candidates_selected: 0,
            candidates_duplicate: 0,
            candidates_existing: 0,
            candidates_missing_source: 0,
            candidates_stale: 0,
            candidates_withdrawn: 0,
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
                .find_active_page(PeriodicSearchFilterCandidatePageRequest {
                    after,
                    page_size: self.policy.filter_page_size.get(),
                    eligible_at_or_before: window_end,
                })
                .await
                .map_err(candidate_read_error)?;
            if page.is_empty() {
                break;
            }
            after = page.last().map(|candidate| candidate.search_filter_id);
            for candidate in page {
                report.filters_selected += 1;
                let mut completed = false;
                let mut failed = false;
                let mut filter_candidates_scanned = 0;
                let mut filter_matches_inserted = 0;
                let mut filter_matches_duplicate = 0;
                let mut selection_tx = self.unit_of_work.begin().await.map_err(|source| {
                    RunPeriodicSearchFilterMatchingError::BeginFinalTransactionFailed {
                        source: box_error(source),
                    }
                })?;
                let mut selected = self
                    .progress
                    .in_transaction(&mut selection_tx)
                    .load_selection(window_end, candidate.search_filter_id)
                    .await
                    .map_err(progress_error)?;
                selection_tx.commit().await.map_err(|source| {
                    RunPeriodicSearchFilterMatchingError::CommitFinalTransactionFailed {
                        source: box_error(source),
                    }
                })?;
                for attempt in 1..=self.policy.max_attempts.get() {
                    report.filter_attempts += 1;
                    let mut attempt_report = FilterAttemptReport::default();
                    let result = self
                        .process_filter(
                            &candidate,
                            &snapshot,
                            window_end,
                            &mut selected,
                            &mut attempt_report,
                        )
                        .instrument(tracing::info_span!("periodic_filter_attempt", attempt))
                        .await;
                    report.merge_attempt(&attempt_report);
                    filter_candidates_scanned += attempt_report.candidates_scanned;
                    filter_matches_inserted += attempt_report.matches_inserted;
                    filter_matches_duplicate += attempt_report.matches_duplicate;
                    match result {
                        Ok(FilterOutcome::Completed) => {
                            completed = true;
                            break;
                        }
                        Ok(FilterOutcome::AlreadyCovered) => {
                            report.filters_already_covered += 1;
                            tracing::info!(search_filter_id = %candidate.search_filter_id, outcome = "already_covered", "search_filter.periodic_match.filter_noop");
                            completed = true;
                            break;
                        }
                        Ok(FilterOutcome::ChangedOrInactive) => {
                            report.filters_changed_or_inactive += 1;
                            tracing::info!(search_filter_id = %candidate.search_filter_id, "search_filter.periodic_match.filter_changed");
                            completed = true;
                            break;
                        }
                        Ok(FilterOutcome::ProgressSuperseded) => {
                            report.filters_progress_superseded += 1;
                            tracing::info!(search_filter_id = %candidate.search_filter_id, outcome = "progress_superseded", "search_filter.periodic_match.filter_noop");
                            completed = true;
                            break;
                        }
                        Ok(FilterOutcome::InvalidPersistedState) => {
                            mark_invalid_persisted_state(&mut report);
                            tracing::error!(search_filter_id = %candidate.search_filter_id, outcome = "invalid_persisted_state", "search_filter.periodic_match.filter_invalid_persisted_state");
                            failed = true;
                            break;
                        }
                        Ok(FilterOutcome::Retryable) => {
                            if attempt == self.policy.max_attempts.get() {
                                report.filters_failed += 1;
                                failed = true;
                            } else {
                                report.filters_retried += 1;
                                retry_delay(attempt).await;
                            }
                        }
                        Err(error) if error.retry_class() == FilterRetryClass::Retryable => {
                            if attempt == self.policy.max_attempts.get() {
                                report.filters_failed += 1;
                                tracing::error!(search_filter_id = %candidate.search_filter_id, attempt, error = %error, "search_filter.periodic_match.filter_failed");
                                failed = true;
                            } else {
                                report.filters_retried += 1;
                                tracing::warn!(search_filter_id = %candidate.search_filter_id, attempt, error = %error, "search_filter.periodic_match.filter_retry");
                                retry_delay(attempt).await;
                            }
                        }
                        Err(error) => {
                            report.filters_failed += 1;
                            tracing::error!(search_filter_id = %candidate.search_filter_id, attempt, error = %error, "search_filter.periodic_match.filter_failed");
                            failed = true;
                            break;
                        }
                    }
                }
                let mut run_tx = self.unit_of_work.begin().await.map_err(|source| {
                    RunPeriodicSearchFilterMatchingError::BeginFinalTransactionFailed {
                        source: box_error(source),
                    }
                })?;
                self.progress
                    .in_transaction(&mut run_tx)
                    .record_filter_attempt(window_end, candidate.search_filter_id, completed)
                    .await
                    .map_err(progress_error)?;
                if completed {
                    self.digests
                        .in_transaction(&mut run_tx)
                        .finalize_for_user(candidate.user_id, window_end)
                        .await
                        .map_err(|source| {
                            RunPeriodicSearchFilterMatchingError::DigestFinalizationFailed {
                                source: box_error(source),
                            }
                        })?;
                }
                run_tx.commit().await.map_err(|source| {
                    RunPeriodicSearchFilterMatchingError::CommitFinalTransactionFailed {
                        source: box_error(source),
                    }
                })?;
                if completed {
                    report.filters_completed += 1;
                    tracing::info!(
                        search_filter_id = %candidate.search_filter_id,
                        filter_candidates_scanned,
                        filter_matches_inserted,
                        filter_matches_duplicate,
                        run_matches_inserted_total = report.matches_inserted,
                        "search_filter.periodic_match.filter_completed"
                    );
                } else if !failed {
                    report.filters_failed += 1;
                    tracing::warn!(
                        search_filter_id = %candidate.search_filter_id,
                        "search_filter.periodic_match.filter_failed"
                    );
                }
            }
        }
        let mut run_tx = self.unit_of_work.begin().await.map_err(|source| {
            RunPeriodicSearchFilterMatchingError::BeginFinalTransactionFailed {
                source: box_error(source),
            }
        })?;
        self.progress
            .in_transaction(&mut run_tx)
            .finish_run(window_end)
            .await
            .map_err(progress_error)?;
        run_tx.commit().await.map_err(|source| {
            RunPeriodicSearchFilterMatchingError::CommitFinalTransactionFailed {
                source: box_error(source),
            }
        })?;
        tracing::info!(
            window_end = %report.window_end,
            filters_selected = report.filters_selected,
            filters_completed = report.filters_completed,
            filters_failed = report.filters_failed,
            candidates_scanned = report.candidates_scanned,
            candidates_withdrawn = report.candidates_withdrawn,
            candidates_selected = report.candidates_selected,
            classification_requests = report.classification_requests,
            candidates_accepted = report.candidates_accepted,
            candidates_rejected = report.candidates_rejected,
            candidates_duplicate = report.candidates_duplicate,
            candidates_existing = report.candidates_existing,
            candidates_stale = report.candidates_stale,
            candidates_missing_source = report.candidates_missing_source,
            filters_candidate_cap = report.filters_candidate_cap,
            filters_scan_cap = report.filters_scan_cap,
            permanent_evaluation_failures = report.permanent_evaluation_failures,
            retryable_evaluation_failures = report.retryable_evaluation_failures,
            matches_inserted = report.matches_inserted,
            matches_duplicate = report.matches_duplicate,
            "search_filter.periodic_match.completed"
        );
        Ok(RunPeriodicSearchFilterMatchingOutcome::Applied(Box::new(
            report,
        )))
    }

    #[tracing::instrument(skip_all, fields(search_filter_id = %filter.search_filter_id, filter_version = filter.version, window_end = %window_end))]
    async fn process_filter(
        &self,
        filter: &PeriodicSearchFilterCandidate,
        snapshot: &FxRateSnapshot,
        window_end: OffsetDateTime,
        selected: &mut Option<Vec<ProductListingSearchFilterMatchSourceRef>>,
        report: &mut FilterAttemptReport,
    ) -> Result<FilterOutcome, RunPeriodicSearchFilterMatchingError> {
        if window_end <= filter.matched_through {
            return Ok(FilterOutcome::AlreadyCovered);
        }
        let semantic = has_semantic_intent(&filter.search);
        let enhanced = filter.search.enhanced_search_description.is_some();
        let embedding = if enhanced {
            match filter_embedding(filter) {
                Ok(embedding) => Some(embedding),
                Err(outcome) => return Ok(outcome),
            }
        } else {
            None
        };
        let Some(search) = periodic_search(filter, window_end, self.policy.replay_overlap) else {
            return self
                .commit_filter(filter, window_end, Vec::new(), true, report)
                .await;
        };
        let price_filter_plan = ProductListingPriceFilterPlan::compile(
            snapshot.clone(),
            search.currency,
            search.price_query,
        )
        .map_err(|source: FxRateSnapshotError| {
            RunPeriodicSearchFilterMatchingError::FxSnapshotInvalid {
                source: box_error(source),
            }
        })?;
        let compiled_search = CompiledProductListingSearch::new(search, price_filter_plan.clone())
            .map_err(
                |source| RunPeriodicSearchFilterMatchingError::FxSnapshotInvalid {
                    source: box_error(source),
                },
            )?;
        let request = ProductListingSearchReadRequest {
            compiled_search,
            sort: None,
            cursor: Some(Cursor {
                size: self.policy.hybrid_scan_limit.get() as u64,
                search_after: None,
            }),
        };
        let sources = if let Some(refs) = selected.as_ref() {
            self.load_sources(refs).await?
        } else {
            let result = match embedding {
                Some(embedding) => {
                    self.product_listings
                        .search_hybrid(&request, embedding)
                        .await
                }
                None => self.product_listings.search(&request).await,
            }
            .map_err(product_search_error)?;
            let scan_count = result.items.len().min(self.policy.hybrid_scan_limit.get());
            report.candidates_scanned += scan_count;
            let scan_cap = scan_count == self.policy.hybrid_scan_limit.get();
            let mut seen = HashSet::new();
            let candidates = result
                .items
                .into_iter()
                .take(scan_count)
                .filter(|item| {
                    if !seen.insert(item.product_listing_id) {
                        report.candidates_duplicate += 1;
                        false
                    } else if is_withdrawn_candidate(item.lifecycle) {
                        report.candidates_withdrawn += 1;
                        false
                    } else {
                        true
                    }
                })
                .collect::<Vec<_>>();
            let ids = candidates
                .iter()
                .map(|item| item.product_listing_id)
                .collect::<Vec<_>>();
            let existing = self
                .existing_matches
                .find_existing_product_listing_ids(filter.search_filter_id, &ids)
                .await
                .map_err(|source| {
                    RunPeriodicSearchFilterMatchingError::ExistingMatchReadFailed {
                        source: box_error(source),
                    }
                })?;
            report.candidates_existing += existing.len();
            let refs = candidates
                .into_iter()
                .filter(|item| !existing.contains(&item.product_listing_id))
                .map(|item| ProductListingSearchFilterMatchSourceRef {
                    product_listing_id: item.product_listing_id,
                    event_id: item.event_id,
                })
                .collect::<Vec<_>>();
            // Authoritative cheap skips precede the classification budget.
            let sources = self.load_sources(&refs).await?;
            let mut eligible = Vec::new();
            for reference in refs {
                if eligible.len() == self.policy.evaluation_limit.get() {
                    break;
                }
                if eligible_source(&reference, &sources, report)?.is_some() {
                    eligible.push(reference);
                }
            }
            report.candidates_selected += eligible.len();
            if eligible.len() == self.policy.evaluation_limit.get() {
                report.filters_candidate_cap += 1;
            }
            if scan_cap {
                report.filters_scan_cap += 1;
            }
            tracing::info!(search_filter_id = %filter.search_filter_id,
                filter_version = filter.version, window_end = %window_end,
                policy_version = search_filter_core::MatchingEvidencePolicy::VERSION,
                evidence_policy = filter.matching_evidence_policy.as_str(),
                retrieved = scan_count, selected = eligible.len(),
                stop_reason = if eligible.len() == self.policy.evaluation_limit.get() { "candidate_cap" }
                    else if scan_cap { "retrieval_scan_cap" } else { "candidate_exhaustion" },
                retrieval_scan_cap_reached = scan_cap, "search_filter.periodic_match.selected");
            let mut selection_tx = self.unit_of_work.begin().await.map_err(|source| {
                RunPeriodicSearchFilterMatchingError::BeginFinalTransactionFailed {
                    source: box_error(source),
                }
            })?;
            let frozen = self
                .progress
                .in_transaction(&mut selection_tx)
                .freeze_selection(window_end, filter.search_filter_id, &eligible)
                .await
                .map_err(progress_error)?;
            selection_tx.commit().await.map_err(|source| {
                RunPeriodicSearchFilterMatchingError::CommitFinalTransactionFailed {
                    source: box_error(source),
                }
            })?;
            let changed = frozen != eligible;
            *selected = Some(frozen);
            if changed {
                self.load_sources(
                    selected.as_ref().ok_or(
                        RunPeriodicSearchFilterMatchingError::ProductListingSourceMismatch,
                    )?,
                )
                .await?
            } else {
                sources
            }
        };
        let refs = selected
            .as_ref()
            .ok_or(RunPeriodicSearchFilterMatchingError::ProductListingSourceMismatch)?;
        let mut evaluations = Vec::new();
        let mut deterministic = Vec::new();
        for reference in refs {
            if let Some(source) = eligible_source(reference, &sources, report)? {
                if semantic {
                    evaluations.push(ProductListingMatchEvaluationRequest {
                        key: *reference,
                        product: source,
                        search: &filter.search,
                        evidence_policy: filter.matching_evidence_policy,
                        correlation_id: Some(format!(
                            "{}:{}:{}:{}:{}",
                            filter.search_filter_id,
                            filter.version,
                            window_end.unix_timestamp(),
                            reference.product_listing_id,
                            reference.event_id
                        )),
                    });
                } else {
                    deterministic.push(
                        crate::product_match_evaluator::ProductListingMatchEvaluationResult {
                            key: *reference,
                            outcome: ProductListingMatchEvaluationOutcome::Matched,
                        },
                    );
                }
            }
        }
        report.classification_requests += evaluations.len();
        let evaluations = evaluate_product_matches(
            &self.evaluator,
            evaluations,
            self.policy.classification_concurrency,
            self.policy.classification_batch_candidates,
            self.policy.should_show_threshold,
        )
        .await;
        let mut retryable = false;
        let mut accepted = Vec::new();
        for evaluation in evaluations.into_iter().chain(deterministic) {
            match evaluation.outcome {
                ProductListingMatchEvaluationOutcome::Matched => {
                    report.candidates_accepted += 1;
                    let source = sources.get(&evaluation.key).ok_or(
                        RunPeriodicSearchFilterMatchingError::ProductListingSourceMismatch,
                    )?;
                    let valuation =
                        filter
                            .search
                            .price_query
                            .as_ref()
                            .map(|_| PriceMatchValuation {
                                basis: if has_applicable_sale_observation(source) {
                                    ProductListingPriceValuationBasis::SaleObservation
                                } else {
                                    ProductListingPriceValuationBasis::Current
                                },
                                fx_rate_id: if let Some(observation) =
                                    applicable_sale_observation(source)
                                {
                                    observation.fx_rate_id()
                                } else {
                                    price_filter_plan.fx_rate_id
                                },
                            });
                    accepted.push(SearchFilterProductListingMatch {
                        user_id: filter.user_id,
                        user_search_filter_id: filter.search_filter_id,
                        user_search_filter_name: Some(filter.name.clone()),
                        product_listing_id: source.product_listing_id,
                        origin_event_id: source.event_id,
                        price_match_valuation: valuation,
                        shipping_match_valuation: filter
                            .search
                            .shipping
                            .as_ref()
                            .filter(|query| query.fx_converted())
                            .and_then(|query| query.maximum_per_item())
                            .map(
                                |maximum_per_item| search_filter_core::ShippingMatchValuation {
                                    fx_rate_id: snapshot.id(),
                                    captured_at: snapshot.captured_at(),
                                    maximum_per_item,
                                },
                            ),
                        feedback: None,
                    });
                }
                ProductListingMatchEvaluationOutcome::Rejected => report.candidates_rejected += 1,
                ProductListingMatchEvaluationOutcome::PermanentFailure(_) => {
                    report.permanent_evaluation_failures += 1
                }
                ProductListingMatchEvaluationOutcome::RetryableFailure(_) => {
                    report.retryable_evaluation_failures += 1;
                    retryable = true;
                }
            }
        }
        if retryable || report.permanent_evaluation_failures > 0 {
            tracing::warn!(search_filter_id = %filter.search_filter_id, selected = refs.len(),
                stop_reason = "technical_incomplete", retryable_failures = report.retryable_evaluation_failures,
                permanent_failures = report.permanent_evaluation_failures,
                advance_progress = !retryable,
                "search_filter.periodic_match.incomplete");
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
        refs: &[ProductListingSearchFilterMatchSourceRef],
    ) -> Result<
        HashMap<ProductListingSearchFilterMatchSourceRef, ProductListingSearchFilterMatchSource>,
        RunPeriodicSearchFilterMatchingError,
    > {
        if refs.is_empty() {
            return Ok(HashMap::new());
        }
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            RunPeriodicSearchFilterMatchingError::BeginProductListingSourceTransactionFailed {
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
            RunPeriodicSearchFilterMatchingError::CommitProductListingSourceTransactionFailed {
                source: box_error(source),
            }
        })?;
        Ok(sources)
    }

    async fn commit_filter(
        &self,
        filter: &PeriodicSearchFilterCandidate,
        window_end: OffsetDateTime,
        matches: Vec<SearchFilterProductListingMatch>,
        advance_progress: bool,
        report: &mut FilterAttemptReport,
    ) -> Result<FilterOutcome, RunPeriodicSearchFilterMatchingError> {
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            RunPeriodicSearchFilterMatchingError::BeginFinalTransactionFailed {
                source: box_error(source),
            }
        })?;
        let matched_through = match self
            .progress
            .in_transaction(&mut tx)
            .lock_and_read(
                filter.search_filter_id,
                filter.version,
                filter.created,
                window_end,
            )
            .await
            .map_err(progress_error)?
        {
            PeriodicSearchFilterProgressLockOutcome::Current { matched_through } => matched_through,
            PeriodicSearchFilterProgressLockOutcome::AlreadyCovered => {
                return Ok(FilterOutcome::AlreadyCovered);
            }
            PeriodicSearchFilterProgressLockOutcome::ChangedOrInactive => {
                return Ok(FilterOutcome::ChangedOrInactive);
            }
        };
        if matched_through != filter.matched_through {
            return Ok(FilterOutcome::ProgressSuperseded);
        }
        if window_end <= matched_through {
            return Ok(FilterOutcome::AlreadyCovered);
        }
        let refs = matches
            .iter()
            .map(|item| ProductListingCurrentEventRef {
                product_listing_id: item.product_listing_id,
                expected_event_id: item.origin_event_id,
            })
            .collect::<Vec<_>>();
        let checks = self
            .current_event_guard
            .in_transaction(&mut tx)
            .lock_and_check_all(&refs)
            .await
            .map_err(|source| {
                RunPeriodicSearchFilterMatchingError::ProductListingCurrentEventCheckFailed {
                    source: box_error(source),
                }
            })?;
        let matches = matches
            .into_iter()
            .filter(|item| {
                let current = checks.get(&ProductListingCurrentEventRef {
                    product_listing_id: item.product_listing_id,
                    expected_event_id: item.origin_event_id,
                }) == Some(&ProductListingCurrentEventCheck::Current);
                if !current {
                    report.candidates_stale += 1;
                }
                current
            })
            .collect::<Vec<_>>();
        let persisted = if matches.is_empty() {
            Default::default()
        } else {
            self.matches
                .in_transaction(&mut tx)
                .insert_for_run(&matches, window_end)
                .await
                .map_err(match_write_error)?
        };
        if advance_progress {
            let progress = self
                .progress
                .in_transaction(&mut tx)
                .compare_and_set(filter.search_filter_id, matched_through, window_end)
                .await
                .map_err(progress_error)?;
            match progress {
                PeriodicSearchFilterProgressWriteOutcome::Advanced => {}
                PeriodicSearchFilterProgressWriteOutcome::AlreadyCovered => {
                    return Ok(FilterOutcome::AlreadyCovered);
                }
                PeriodicSearchFilterProgressWriteOutcome::Superseded => {
                    return Ok(FilterOutcome::ProgressSuperseded);
                }
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
    AlreadyCovered,
    ChangedOrInactive,
    ProgressSuperseded,
    InvalidPersistedState,
    Retryable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilterRetryClass {
    Retryable,
    Permanent,
}

impl RunPeriodicSearchFilterMatchingError {
    const fn retry_class(&self) -> FilterRetryClass {
        match self {
            Self::ProductListingSearchFailed { .. }
            | Self::ExistingMatchReadFailed { .. }
            | Self::BeginProductListingSourceTransactionFailed { .. }
            | Self::ProductListingSourceReadFailed { .. }
            | Self::CommitProductListingSourceTransactionFailed { .. }
            | Self::BeginFinalTransactionFailed { .. }
            | Self::ProgressFailed { .. }
            | Self::ProductListingCurrentEventCheckFailed { .. }
            | Self::MatchPersistenceFailed { .. }
            | Self::CommitFinalTransactionFailed { .. }
            | Self::DigestFinalizationFailed { .. } => FilterRetryClass::Retryable,
            Self::InvalidPolicy
            | Self::RunLockFailed { .. }
            | Self::RunLockReleaseFailed { .. }
            | Self::BeginFxSnapshotTransactionFailed { .. }
            | Self::FxSnapshotReadFailed { .. }
            | Self::FxSnapshotInvalid { .. }
            | Self::FxSnapshotNotFound
            | Self::CommitFxSnapshotTransactionFailed { .. }
            | Self::CandidateReadFailed { .. }
            | Self::CandidateStateInvalid { .. }
            | Self::ProductListingSearchResultInvalid { .. }
            | Self::ProductListingSourceStateInvalid { .. }
            | Self::ProductListingSourceMismatch
            | Self::MatchStateInvalid { .. } => FilterRetryClass::Permanent,
        }
    }
}

async fn retry_delay(attempt: usize) {
    let delay = if attempt == 1 {
        std::time::Duration::from_millis(250)
    } else {
        std::time::Duration::from_secs(1)
    };
    tokio::time::sleep(delay).await;
}

fn has_semantic_intent(search: &ProductListingSearch) -> bool {
    search.enhanced_search_description.is_some()
        || search
            .product_listing_query
            .iter()
            .any(|query| !query.trim().is_empty())
}

fn eligible_source<'a>(
    reference: &ProductListingSearchFilterMatchSourceRef,
    sources: &'a HashMap<
        ProductListingSearchFilterMatchSourceRef,
        ProductListingSearchFilterMatchSource,
    >,
    report: &mut FilterAttemptReport,
) -> Result<Option<&'a ProductListingSearchFilterMatchSource>, RunPeriodicSearchFilterMatchingError>
{
    match sources.get(reference) {
        None => {
            report.candidates_missing_source += 1;
            Ok(None)
        }
        Some(source)
            if source.product_listing_id != reference.product_listing_id
                || source.event_id != reference.event_id =>
        {
            Err(RunPeriodicSearchFilterMatchingError::ProductListingSourceMismatch)
        }
        Some(source) if source.current_event_id != reference.event_id => {
            report.candidates_stale += 1;
            Ok(None)
        }
        Some(source) if is_withdrawn_candidate(source.lifecycle) => {
            report.candidates_withdrawn += 1;
            Ok(None)
        }
        Some(source) => Ok(Some(source)),
    }
}

fn filter_embedding(filter: &PeriodicSearchFilterCandidate) -> Result<&[f32], FilterOutcome> {
    filter
        .embedding
        .as_deref()
        .filter(|values| {
            values.len() == embedding::EMBEDDING_DIMENSIONS
                && values.iter().all(|value| value.is_finite())
                && values.iter().any(|value| *value != 0.0)
        })
        .ok_or(FilterOutcome::InvalidPersistedState)
}

fn mark_invalid_persisted_state(report: &mut PeriodicSearchFilterMatchingReport) {
    report.filters_failed += 1;
    report.filters_invalid_persisted_state += 1;
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
) -> Option<ProductListingSearch> {
    // Every persisted listing-state change, enrichment included, advances listing.updated, so
    // one interval applies to every filter type; FX changes alone never reopen old listings.
    let replay_start = filter
        .matched_through
        .checked_sub(replay_overlap)
        .unwrap_or(filter.created)
        .max(filter.created);
    let min = filter
        .search
        .updated_query
        .as_ref()
        .and_then(|range| range.min)
        .map_or(replay_start, |authored| authored.max(replay_start));
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
fn product_search_error(
    error: ProductListingSearchReadError,
) -> RunPeriodicSearchFilterMatchingError {
    match error {
        ProductListingSearchReadError::ProductListingSearchQueryFailed { source } => {
            RunPeriodicSearchFilterMatchingError::ProductListingSearchFailed { source }
        }
        ProductListingSearchReadError::ProductListingSearchReadModelInvalid { source } => {
            RunPeriodicSearchFilterMatchingError::ProductListingSearchResultInvalid { source }
        }
        error @ ProductListingSearchReadError::InvalidCursor => {
            RunPeriodicSearchFilterMatchingError::ProductListingSearchResultInvalid {
                source: box_error(error),
            }
        }
    }
}
fn is_withdrawn_candidate(lifecycle: ListingLifecycle) -> bool {
    lifecycle == ListingLifecycle::Withdrawn
}

fn applicable_sale_observation(
    source: &ProductListingSearchFilterMatchSource,
) -> Option<product_listing_core::product_listing::ListingSaleObservation> {
    if source.availability == Some(ListingAvailability::SoldOut)
        || source.lifecycle == ListingLifecycle::Withdrawn
    {
        source.sale_observation
    } else {
        None
    }
}

fn has_applicable_sale_observation(source: &ProductListingSearchFilterMatchSource) -> bool {
    applicable_sale_observation(source).is_some()
}

fn product_source_error(
    error: ProductListingSearchFilterMatchSourceReadError,
) -> RunPeriodicSearchFilterMatchingError {
    match error {
        ProductListingSearchFilterMatchSourceReadError::InvalidPersistedState { source } => {
            RunPeriodicSearchFilterMatchingError::ProductListingSourceStateInvalid { source }
        }
        error => RunPeriodicSearchFilterMatchingError::ProductListingSourceReadFailed {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        ExistingSearchFilterMatchReadError, PeriodicSearchFilterMatchingRunLease,
        PeriodicSearchFilterProgressLockOutcome, SearchFilterMatchPersistOutcome,
    };
    use crate::test_digests::TestDigests;
    use application::transaction::TransactionError;
    use classifier_model::{
        ClassificationError, ClassificationRequest, ClassificationResponse, Probability,
    };
    use domain_primitives::event_id::EventId;
    use fxrate_core::{
        FX_RATE_SCALE, FxRateGeneration, FxRateId, FxRateQuote, FxRateSource, NewFxRateSnapshot,
    };
    use fxrate_service::ports::FxRateSnapshotInsertOutcome;
    use indexmap::IndexSet;
    use listing_source_core::ListingSourceId;
    use localization::Language;
    use money::Currency;
    use product_listing_core::{
        listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
        product_listing_slug_id::ProductListingSlugId, source_listing_id::SourceListingId,
    };
    use product_listing_service::use_cases::queries::search_product_listings::{
        ProductListingSearchItem, ProductListingSearchReadResult,
        ProductListingSummaryPriceValuation,
    };
    use search_filter_core::{
        search_filter_state::SearchFilterState, user_search_filter_name::UserSearchFilterName,
    };
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};
    use strum::IntoEnumIterator;
    use url::Url;
    use user_core::user_id::UserId;

    #[derive(Debug)]
    struct State {
        lock_outcome: PeriodicSearchFilterProgressLockOutcome,
        current_event_check: ProductListingCurrentEventCheck,
        product_listing_search_result: Option<ProductListingSearchReadResult>,
        policy_change_during_evaluation: bool,
        evaluator_calls: usize,
        evaluator_requests: Vec<ClassificationRequest>,
        evaluator_results:
            std::collections::VecDeque<Result<ClassificationResponse, ClassificationError>>,
        sources: HashMap<
            ProductListingSearchFilterMatchSourceRef,
            ProductListingSearchFilterMatchSource,
        >,
        existing: HashSet<ProductListingId>,
        match_writer_calls: usize,
        commits: usize,
        event_checks: usize,
        persisted: Vec<SearchFilterProductListingMatch>,
        checkpoints: usize,
    }

    #[derive(Clone)]
    struct FakeUnitOfWork(Arc<Mutex<State>>);

    struct FakeTransaction(Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = self.0.lock().map_err(|_| {
                TransactionError::CommitFailed(application::error::static_error(
                    "test transaction failure",
                ))
            })?;
            state.commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            Ok(FakeTransaction(Arc::clone(&self.0)))
        }
    }

    struct NoopRunLock;
    struct NoopRunLease;

    #[async_trait::async_trait]
    impl PeriodicSearchFilterMatchingRunLease for NoopRunLease {
        async fn release(self: Box<Self>) -> Result<(), PeriodicSearchFilterMatchingRunLockError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl PeriodicSearchFilterMatchingRunLock for NoopRunLock {
        async fn try_acquire(
            &self,
        ) -> Result<
            Option<Box<dyn PeriodicSearchFilterMatchingRunLease>>,
            PeriodicSearchFilterMatchingRunLockError,
        > {
            Ok(Some(Box::new(NoopRunLease)))
        }
    }

    #[derive(Default)]
    struct TestCandidates(Vec<PeriodicSearchFilterCandidate>);

    #[async_trait::async_trait]
    impl PeriodicSearchFilterCandidateReader for TestCandidates {
        async fn find_active_page(
            &self,
            request: PeriodicSearchFilterCandidatePageRequest,
        ) -> Result<Vec<PeriodicSearchFilterCandidate>, PeriodicSearchFilterCandidateReadError>
        {
            Ok(self
                .0
                .iter()
                .filter(|filter| {
                    request
                        .after
                        .is_none_or(|after| filter.search_filter_id > after)
                })
                .cloned()
                .collect())
        }
    }

    struct TestFxRates;
    struct ReadingTestFxRates;

    #[async_trait::async_trait]
    impl FxRateSnapshotRepository for ReadingTestFxRates {
        async fn find_latest(
            &mut self,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(None)
        }

        async fn find_latest_at_or_before(
            &mut self,
            _timestamp: OffsetDateTime,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(Some(test_snapshot()))
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
            _snapshot: &NewFxRateSnapshot,
            _source_event_id: &str,
        ) -> Result<FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError> {
            Ok(FxRateSnapshotInsertOutcome::Duplicate)
        }
    }

    impl FxRateSnapshotRepositoryFactory<FakeTransaction> for TestFxRates {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl FxRateSnapshotRepository + 'tx {
            ReadingTestFxRates
        }
    }

    struct NoopProductListings(Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl ProductListingSearchReader for NoopProductListings {
        fn continuation_mode(
            &self,
            _: &serde_json::Value,
            _: product_listing_core::sort_product_listing_field::SortProductListingField,
        ) -> Result<
            product_listing_service::ports::product_listing_search_reader::ProductListingSearchMode,
            ProductListingSearchReadError,
        > {
            unreachable!("periodic matching does not resume search continuations")
        }

        async fn search(
            &self,
            _request: &ProductListingSearchReadRequest,
        ) -> Result<
            product_listing_service::use_cases::queries::search_product_listings::ProductListingSearchReadResult,
            ProductListingSearchReadError,
        >{
            self.search_hybrid(_request, &[]).await
        }

        async fn search_hybrid(
            &self,
            _request: &ProductListingSearchReadRequest,
            _embedding: &[f32],
        ) -> Result<
            product_listing_service::use_cases::queries::search_product_listings::ProductListingSearchReadResult,
            ProductListingSearchReadError,
        >{
            self.0
                .lock()
                .map(|state| match &state.product_listing_search_result {
                    Some(result) => result.clone(),
                    None => Default::default(),
                })
                .map_err(
                    |_| ProductListingSearchReadError::ProductListingSearchQueryFailed {
                        source: application::error::static_error("fake search state poisoned"),
                    },
                )
        }
    }

    struct NoopExistingMatches(Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl ExistingSearchFilterMatchReader for NoopExistingMatches {
        async fn find_existing_product_listing_ids(
            &self,
            _search_filter_id: search_filter_core::user_search_filter_id::UserSearchFilterId,
            _product_listing_ids: &[ProductListingId],
        ) -> Result<HashSet<ProductListingId>, ExistingSearchFilterMatchReadError> {
            Ok(self.0.lock().unwrap().existing.clone())
        }
    }

    struct NoopSources(Arc<Mutex<State>>);
    struct ReadingNoopSources(Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl ProductListingSearchFilterMatchSourceReader for ReadingNoopSources {
        async fn find_source(
            &mut self,
            _event_id: EventId,
            _product_listing_id: ProductListingId,
        ) -> Result<
            Option<ProductListingSearchFilterMatchSource>,
            ProductListingSearchFilterMatchSourceReadError,
        > {
            Ok(self
                .0
                .lock()
                .unwrap()
                .sources
                .get(&ProductListingSearchFilterMatchSourceRef {
                    event_id: _event_id,
                    product_listing_id: _product_listing_id,
                })
                .cloned())
        }
    }

    impl ProductListingSearchFilterMatchSourceReaderFactory<FakeTransaction> for NoopSources {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ProductListingSearchFilterMatchSourceReader + 'tx {
            ReadingNoopSources(Arc::clone(&self.0))
        }
    }

    struct NoopEvaluator(Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl ClassifierModel for NoopEvaluator {
        fn request_limits(&self) -> Option<classifier_model::ClassificationRequestLimits> {
            Some(classifier_model::ClassificationRequestLimits {
                max_questions: 64,
                max_images: 4,
                max_text_tokens: 1_000_000,
                max_context_tokens: 1_000_000,
                context_reserve_tokens: 1024,
                tokens_per_image: 1027,
                max_payload_bytes: 13 * 1024 * 1024,
            })
        }
        async fn classify(
            &self,
            _request: ClassificationRequest,
        ) -> Result<ClassificationResponse, ClassificationError> {
            let mut state = self.0.lock().unwrap();
            if state.policy_change_during_evaluation {
                state.lock_outcome = PeriodicSearchFilterProgressLockOutcome::ChangedOrInactive;
            }
            state.evaluator_calls += 1;
            state.evaluator_requests.push(_request.clone());
            if let Some(candidates) = _request.state["candidates"].as_array() {
                let mut answers = std::collections::BTreeMap::new();
                for candidate in candidates {
                    let response = state.evaluator_results.pop_front().unwrap()?;
                    for (question, probability) in response.answers {
                        let id = classifier_model::QuestionId::new(format!(
                            "{}.{}",
                            candidate["id"].as_str().unwrap(),
                            question.as_str()
                        ))
                        .unwrap();
                        answers.insert(id, probability);
                    }
                }
                return Ok(ClassificationResponse {
                    answers,
                    usage: Default::default(),
                    diagnostics: Default::default(),
                });
            }
            state
                .evaluator_results
                .pop_front()
                .unwrap_or(Err(ClassificationError::PermanentCandidateFailure))
        }
    }

    #[derive(Clone)]
    struct FakeCurrentEvents(Arc<Mutex<State>>);

    struct CheckingCurrentEvents<'a>(&'a Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl ProductListingCurrentEventGuard for CheckingCurrentEvents<'_> {
        async fn lock_and_check(
            &mut self,
            _product_listing_id: ProductListingId,
            _expected_event_id: EventId,
        ) -> Result<
            ProductListingCurrentEventCheck,
            product_listing_service::ports::ProductListingCurrentEventCheckError,
        > {
            let mut state = self.0.lock().map_err(|_| {
                product_listing_service::ports::ProductListingCurrentEventCheckError::CheckFailed {
                    source: box_error(std::io::Error::other("test mutex poisoned")),
                }
            })?;
            state.event_checks += 1;
            Ok(state.current_event_check)
        }
    }

    impl ProductListingCurrentEventGuardFactory<FakeTransaction> for FakeCurrentEvents {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ProductListingCurrentEventGuard + 'tx {
            CheckingCurrentEvents(&self.0)
        }
    }

    #[derive(Clone)]
    struct FakeMatches(Arc<Mutex<State>>);

    struct WritingMatches<'a>(&'a Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl SearchFilterMatchWriter for WritingMatches<'_> {
        async fn insert_for_run(
            &mut self,
            matches: &[SearchFilterProductListingMatch],
            _: OffsetDateTime,
        ) -> Result<crate::ports::SearchFilterMatchBatchPersistResult, SearchFilterMatchWriteError>
        {
            self.insert_all_if_absent(matches).await
        }

        async fn insert_if_absent(
            &mut self,
            product_match: &SearchFilterProductListingMatch,
        ) -> Result<SearchFilterMatchPersistOutcome, SearchFilterMatchWriteError> {
            let mut state =
                self.0
                    .lock()
                    .map_err(|_| SearchFilterMatchWriteError::WriteFailed {
                        source: box_error(std::io::Error::other("test mutex poisoned")),
                    })?;
            state.match_writer_calls += 1;
            if !state.existing.insert(product_match.product_listing_id) {
                return Ok(SearchFilterMatchPersistOutcome::AlreadyExists);
            }
            state.persisted.push(product_match.clone());
            Ok(SearchFilterMatchPersistOutcome::Inserted)
        }
    }

    impl SearchFilterMatchWriterFactory<FakeTransaction> for FakeMatches {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl SearchFilterMatchWriter + 'tx {
            WritingMatches(&self.0)
        }
    }

    #[derive(Clone)]
    struct FakeProgress(Arc<Mutex<State>>);

    struct LockingProgress<'a>(&'a Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl PeriodicSearchFilterProgress for LockingProgress<'_> {
        async fn load_selection(
            &mut self,
            _: OffsetDateTime,
            _: search_filter_core::user_search_filter_id::UserSearchFilterId,
        ) -> Result<
            Option<Vec<ProductListingSearchFilterMatchSourceRef>>,
            PeriodicSearchFilterProgressError,
        > {
            Ok(None)
        }
        async fn freeze_selection(
            &mut self,
            _: OffsetDateTime,
            _: search_filter_core::user_search_filter_id::UserSearchFilterId,
            refs: &[ProductListingSearchFilterMatchSourceRef],
        ) -> Result<Vec<ProductListingSearchFilterMatchSourceRef>, PeriodicSearchFilterProgressError>
        {
            Ok(refs.to_vec())
        }

        async fn begin_run(
            &mut self,
            end: OffsetDateTime,
        ) -> Result<OffsetDateTime, PeriodicSearchFilterProgressError> {
            Ok(end)
        }
        async fn record_filter_attempt(
            &mut self,
            _: OffsetDateTime,
            _: search_filter_core::user_search_filter_id::UserSearchFilterId,
            _: bool,
        ) -> Result<(), PeriodicSearchFilterProgressError> {
            Ok(())
        }
        async fn finish_run(
            &mut self,
            _: OffsetDateTime,
        ) -> Result<(), PeriodicSearchFilterProgressError> {
            Ok(())
        }

        async fn lock_and_read(
            &mut self,
            _search_filter_id: search_filter_core::user_search_filter_id::UserSearchFilterId,
            _expected_version: i64,
            _created: OffsetDateTime,
            _window_end: OffsetDateTime,
        ) -> Result<PeriodicSearchFilterProgressLockOutcome, PeriodicSearchFilterProgressError>
        {
            self.0.lock().map(|state| state.lock_outcome).map_err(|_| {
                PeriodicSearchFilterProgressError::PersistenceFailed {
                    source: box_error(std::io::Error::other("test mutex poisoned")),
                }
            })
        }

        async fn compare_and_set(
            &mut self,
            _search_filter_id: search_filter_core::user_search_filter_id::UserSearchFilterId,
            _expected_matched_through: OffsetDateTime,
            _matched_through: OffsetDateTime,
        ) -> Result<PeriodicSearchFilterProgressWriteOutcome, PeriodicSearchFilterProgressError>
        {
            let mut state = self.0.lock().map_err(|_| {
                PeriodicSearchFilterProgressError::PersistenceFailed {
                    source: box_error(std::io::Error::other("test mutex poisoned")),
                }
            })?;
            state.checkpoints += 1;
            Ok(PeriodicSearchFilterProgressWriteOutcome::Advanced)
        }
    }

    impl PeriodicSearchFilterProgressFactory<FakeTransaction> for FakeProgress {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl PeriodicSearchFilterProgress + 'tx {
            LockingProgress(&self.0)
        }
    }

    type Handler = RunPeriodicSearchFilterMatchingHandler<
        FakeUnitOfWork,
        NoopRunLock,
        TestCandidates,
        TestFxRates,
        NoopProductListings,
        NoopExistingMatches,
        NoopSources,
        NoopEvaluator,
        FakeCurrentEvents,
        FakeMatches,
        FakeProgress,
        TestDigests,
    >;

    fn state(lock_outcome: PeriodicSearchFilterProgressLockOutcome) -> Arc<Mutex<State>> {
        Arc::new(Mutex::new(State {
            lock_outcome,
            current_event_check: ProductListingCurrentEventCheck::Current,
            product_listing_search_result: None,
            policy_change_during_evaluation: false,
            evaluator_calls: 0,
            evaluator_requests: Vec::new(),
            evaluator_results: Default::default(),
            sources: HashMap::new(),
            existing: HashSet::new(),
            match_writer_calls: 0,
            commits: 0,
            event_checks: 0,
            persisted: Vec::new(),
            checkpoints: 0,
        }))
    }

    fn handler(state: Arc<Mutex<State>>) -> Result<Handler, RunPeriodicSearchFilterMatchingError> {
        RunPeriodicSearchFilterMatchingHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            NoopRunLock,
            TestCandidates::default(),
            TestFxRates,
            NoopProductListings(Arc::clone(&state)),
            NoopExistingMatches(Arc::clone(&state)),
            NoopSources(Arc::clone(&state)),
            NoopEvaluator(Arc::clone(&state)),
            FakeCurrentEvents(Arc::clone(&state)),
            FakeMatches(Arc::clone(&state)),
            FakeProgress(state),
            TestDigests::default(),
            PeriodicSearchFilterMatchingPolicy {
                filter_page_size: NonZeroUsize::MIN,
                hybrid_scan_limit: NonZeroUsize::MIN,
                evaluation_limit: NonZeroUsize::MIN,
                classification_concurrency: NonZeroUsize::MIN,
                classification_batch_candidates: NonZeroUsize::MIN,
                should_show_threshold: Probability::new(0.5)
                    .unwrap_or_else(|_| unreachable!("test threshold is valid")),
                max_attempts: NonZeroUsize::MIN,
                projection_lag: Duration::ZERO,
                replay_overlap: Duration::ZERO,
            },
        )
    }

    fn filter() -> PeriodicSearchFilterCandidate {
        PeriodicSearchFilterCandidate {
            search_filter_id: search_filter_core::user_search_filter_id::UserSearchFilterId::new(),
            user_id: UserId::new(),
            name: UserSearchFilterName::from("daily"),
            version: 4,
            state: SearchFilterState::Active,
            search: ProductListingSearch::new(Language::En, Currency::Eur),
            embedding: None,
            matching_evidence_policy:
                search_filter_core::MatchingEvidencePolicy::EnhancedTextAndFourImages,
            created: OffsetDateTime::UNIX_EPOCH,
            matched_through: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn every_filter_variant_uses_the_same_replay_window_and_authored_date_bounds() {
        let shipping = product_listing_core::shipping_search::ShippingSearch::new(
            geo::reference::ReferenceRelease::CURRENT,
            Some(geo::CountryCode::DEU),
            None,
            Some(money::Price::new(
                money::MonetaryAmount::from(1_000_u64),
                Currency::Usd,
            )),
            false,
        )
        .unwrap();
        let geography = geo::core::discovery::GeographicQuery::new(
            geo::reference::ReferenceRelease::CURRENT,
            HashSet::from([geo::CountryCode::DEU]),
            HashSet::new(),
            None,
            None,
            geo::core::discovery::ProximityMode::Strict,
            None,
        )
        .unwrap();
        let variants = [
            (None, None),
            (Some(shipping.clone()), None),
            (None, Some(geography.clone())),
            (Some(shipping), Some(geography)),
        ];
        for (shipping, geography) in variants {
            let mut candidate = filter();
            candidate.search.shipping = shipping;
            candidate.search.geography = geography;
            candidate.matched_through = candidate.created + Duration::days(31);
            let next = candidate.matched_through + Duration::days(1);
            let replay_start = candidate.matched_through - Duration::minutes(5);

            assert_eq!(
                Some(RangeQuery {
                    min: Some(replay_start),
                    max: Some(next),
                }),
                periodic_search(&candidate, next, Duration::minutes(5))
                    .unwrap()
                    .updated_query
            );
            // Old listings outside the replay window stay excluded even when authored bounds
            // would admit them; FX or shipping predicates never widen the interval.
            candidate.search.updated_query = Some(RangeQuery {
                min: Some(candidate.created),
                max: Some(candidate.created + Duration::hours(1)),
            });
            assert!(periodic_search(&candidate, next, Duration::minutes(5)).is_none());
            candidate.search.updated_query = Some(RangeQuery {
                min: Some(next + Duration::hours(1)),
                max: None,
            });
            assert!(periodic_search(&candidate, next, Duration::minutes(5)).is_none());
            candidate.search.updated_query = Some(RangeQuery {
                min: Some(candidate.created),
                max: Some(next + Duration::hours(1)),
            });
            assert_eq!(
                Some(RangeQuery {
                    min: Some(replay_start),
                    max: Some(next)
                }),
                periodic_search(&candidate, next, Duration::minutes(5))
                    .unwrap()
                    .updated_query
            );
            candidate.search.updated_query = Some(RangeQuery {
                min: Some(replay_start + Duration::hours(1)),
                max: Some(next - Duration::hours(1)),
            });
            assert_eq!(
                Some(RangeQuery {
                    min: Some(replay_start + Duration::hours(1)),
                    max: Some(next - Duration::hours(1))
                }),
                periodic_search(&candidate, next, Duration::minutes(5))
                    .unwrap()
                    .updated_query
            );
        }
    }

    #[tokio::test]
    async fn finishing_a_filter_checks_its_users_digest_without_a_notification_job()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state(PeriodicSearchFilterProgressLockOutcome::Current {
            matched_through: OffsetDateTime::UNIX_EPOCH,
        });
        let mut handler = handler(state)?;
        let filter = filter();
        // A search with no new matches must still attempt finalization.
        handler.candidates = TestCandidates(vec![filter.clone()]);
        let checks = handler.digests.checks.clone();
        let end = OffsetDateTime::UNIX_EPOCH + Duration::hours(1);
        let outcome = handler
            .execute(RunPeriodicSearchFilterMatchingCommand { started_at: end })
            .await?;
        let RunPeriodicSearchFilterMatchingOutcome::Applied(report) = outcome else {
            panic!("expected completed run")
        };
        assert_eq!(1, report.filters_completed);
        assert_eq!(0, report.matches_inserted);
        assert_eq!(vec![(filter.user_id, end)], *checks.lock().unwrap());
        Ok(())
    }

    fn geographic_filter() -> PeriodicSearchFilterCandidate {
        let mut filter = filter();
        filter.search.geography = Some(
            geo::core::discovery::GeographicQuery::new(
                geo::reference::ReferenceRelease::CURRENT,
                HashSet::from([geo::CountryCode::DEU]),
                Default::default(),
                None,
                None,
                Default::default(),
                None,
            )
            .unwrap(),
        );
        filter
    }

    fn withdrawn_search_result()
    -> Result<ProductListingSearchReadResult, Box<dyn std::error::Error>> {
        let mut result = ProductListingSearchReadResult::default();
        result.items.push(ProductListingSearchItem {
            geography: Default::default(),
            product_listing_id: ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::raw("withdrawn-a1b2c3")?,
            event_id: EventId::new(),
            listing_source_id: ListingSourceId::new(),
            source_listing_id: SourceListingId::try_from("withdrawn-1")?,
            auction_id: None,
            title: None,
            display_price: None,
            price_valuation: ProductListingSummaryPriceValuation::Current {
                fx_rate_id: FxRateId::new(),
                captured_at: OffsetDateTime::UNIX_EPOCH,
            },
            availability: None,
            lifecycle: ListingLifecycle::Withdrawn,
            url: Url::parse("https://example.test/withdrawn")?,
            images: IndexSet::new(),
            updated: OffsetDateTime::UNIX_EPOCH,
        });
        Ok(result)
    }

    #[tokio::test]
    async fn should_skip_current_withdrawn_candidate_evaluation_and_match_write()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state(PeriodicSearchFilterProgressLockOutcome::Current {
            matched_through: OffsetDateTime::UNIX_EPOCH,
        });
        state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?
            .product_listing_search_result = Some(withdrawn_search_result()?);
        let handler = handler(Arc::clone(&state))?;
        let mut filter = filter();
        filter.embedding = Some(vec![0.1; embedding::EMBEDDING_DIMENSIONS]);
        filter.search.enhanced_search_description = Some(
            product_listing_core::product_listing_search::EnhancedSearchDescription::try_from(
                "withdrawn listing",
            )?,
        );
        let mut attempt_report = FilterAttemptReport::default();

        let outcome = handler
            .process_filter(
                &filter,
                &test_snapshot(),
                OffsetDateTime::UNIX_EPOCH + Duration::seconds(1),
                &mut None,
                &mut attempt_report,
            )
            .await?;

        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(FilterOutcome::Completed, outcome);
        assert_eq!(1, attempt_report.candidates_scanned);
        assert_eq!(1, attempt_report.candidates_withdrawn);
        assert_eq!(0, state.evaluator_calls);
        assert_eq!(0, state.match_writer_calls);
        assert!(state.persisted.is_empty());
        assert_eq!(1, state.checkpoints);
        assert_eq!(2, state.commits);
        Ok(())
    }

    #[test]
    fn should_clamp_first_replay_window_to_filter_creation() {
        let mut candidate = filter();
        candidate.created = OffsetDateTime::UNIX_EPOCH + Duration::hours(10);
        candidate.matched_through = candidate.created;
        let window_end = candidate.created + Duration::hours(1);

        let search = periodic_search(&candidate, window_end, Duration::hours(2));

        assert_eq!(
            Some(RangeQuery {
                min: Some(candidate.created),
                max: Some(window_end),
            }),
            search.and_then(|search| search.updated_query)
        );
    }

    fn test_snapshot() -> FxRateSnapshot {
        NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| {
                FxRateQuote::new(
                    currency,
                    if currency == Currency::Eur {
                        FX_RATE_SCALE
                    } else {
                        1_250_000
                    },
                )
            }),
        )
        .unwrap_or_else(|error| panic!("test snapshot invalid: {error}"))
        .into_persisted(
            FxRateGeneration::try_from(1)
                .unwrap_or_else(|error| panic!("test generation invalid: {error}")),
        )
    }

    fn accepted_match(filter: &PeriodicSearchFilterCandidate) -> SearchFilterProductListingMatch {
        SearchFilterProductListingMatch {
            user_id: filter.user_id,
            user_search_filter_id: filter.search_filter_id,
            user_search_filter_name: Some(filter.name.clone()),
            product_listing_id: ProductListingId::new(),
            origin_event_id: EventId::new(),
            price_match_valuation: None,
            shipping_match_valuation: None,
            feedback: None,
        }
    }

    fn report() -> PeriodicSearchFilterMatchingReport {
        PeriodicSearchFilterMatchingReport {
            window_end: OffsetDateTime::UNIX_EPOCH,
            filters_selected: 0,
            filters_completed: 0,
            filters_already_covered: 0,
            filters_changed_or_inactive: 0,
            filters_progress_superseded: 0,
            filter_attempts: 0,
            filters_retried: 0,
            filters_failed: 0,
            filters_invalid_persisted_state: 0,
            candidates_scanned: 0,
            filters_scan_cap: 0,
            filters_candidate_cap: 0,
            candidates_accepted: 0,
            classification_requests: 0,
            candidates_selected: 0,
            candidates_duplicate: 0,
            candidates_existing: 0,
            candidates_missing_source: 0,
            candidates_stale: 0,
            candidates_withdrawn: 0,
            candidates_rejected: 0,
            permanent_evaluation_failures: 0,
            retryable_evaluation_failures: 0,
            matches_inserted: 0,
            matches_duplicate: 0,
        }
    }

    #[test]
    fn should_report_missing_embedding_without_advancing_progress()
    -> Result<(), RunPeriodicSearchFilterMatchingError> {
        let state = state(PeriodicSearchFilterProgressLockOutcome::Current {
            matched_through: OffsetDateTime::UNIX_EPOCH,
        });
        let filter = filter();
        let mut report = report();

        assert_eq!(
            filter_embedding(&filter),
            Err(FilterOutcome::InvalidPersistedState)
        );
        mark_invalid_persisted_state(&mut report);

        let state = state
            .lock()
            .map_err(|_| RunPeriodicSearchFilterMatchingError::InvalidPolicy)?;
        assert_eq!(report.filters_failed, 1);
        assert_eq!(report.filters_invalid_persisted_state, 1);
        assert_eq!(state.checkpoints, 0);
        assert_eq!(state.commits, 0);
        Ok(())
    }

    #[tokio::test]
    async fn should_skip_all_filter_work_when_window_is_already_covered()
    -> Result<(), RunPeriodicSearchFilterMatchingError> {
        let state = state(PeriodicSearchFilterProgressLockOutcome::Current {
            matched_through: OffsetDateTime::UNIX_EPOCH,
        });
        let handler = handler(Arc::clone(&state))?;
        let filter = geographic_filter();
        let snapshot = test_snapshot();
        let mut attempt_report = FilterAttemptReport::default();

        let equal = handler
            .process_filter(
                &filter,
                &snapshot,
                filter.matched_through,
                &mut None,
                &mut attempt_report,
            )
            .await?;
        let older = handler
            .process_filter(
                &filter,
                &snapshot,
                filter.matched_through - Duration::seconds(1),
                &mut None,
                &mut attempt_report,
            )
            .await?;

        let state = state
            .lock()
            .map_err(|_| RunPeriodicSearchFilterMatchingError::InvalidPolicy)?;
        assert_eq!(FilterOutcome::AlreadyCovered, equal);
        assert_eq!(FilterOutcome::AlreadyCovered, older);
        assert_eq!(0, state.commits);
        assert_eq!(0, state.event_checks);
        assert_eq!(0, state.checkpoints);
        assert_eq!(0, attempt_report.candidates_scanned);
        Ok(())
    }

    #[tokio::test]
    async fn should_report_progress_superseded_when_selected_checkpoint_changes()
    -> Result<(), RunPeriodicSearchFilterMatchingError> {
        let state = state(PeriodicSearchFilterProgressLockOutcome::Current {
            matched_through: OffsetDateTime::UNIX_EPOCH + Duration::seconds(1),
        });
        let handler = handler(Arc::clone(&state))?;
        let filter = filter();
        let mut attempt_report = FilterAttemptReport::default();

        let outcome = handler
            .commit_filter(
                &filter,
                OffsetDateTime::UNIX_EPOCH + Duration::seconds(2),
                vec![accepted_match(&filter)],
                true,
                &mut attempt_report,
            )
            .await?;

        let state = state
            .lock()
            .map_err(|_| RunPeriodicSearchFilterMatchingError::InvalidPolicy)?;
        assert_eq!(FilterOutcome::ProgressSuperseded, outcome);
        assert_eq!(0, state.event_checks);
        assert!(state.persisted.is_empty());
        assert_eq!(0, state.checkpoints);
        assert_eq!(0, state.commits);
        Ok(())
    }

    #[tokio::test]
    async fn should_skip_revision_match_and_checkpoint_when_filter_changed_or_inactive()
    -> Result<(), RunPeriodicSearchFilterMatchingError> {
        let state = state(PeriodicSearchFilterProgressLockOutcome::ChangedOrInactive);
        let handler = handler(Arc::clone(&state))?;
        let filter = filter();
        let mut attempt_report = FilterAttemptReport::default();

        let outcome = handler
            .commit_filter(
                &filter,
                OffsetDateTime::UNIX_EPOCH,
                vec![accepted_match(&filter)],
                true,
                &mut attempt_report,
            )
            .await?;

        let state = state
            .lock()
            .map_err(|_| RunPeriodicSearchFilterMatchingError::InvalidPolicy)?;
        assert_eq!(outcome, FilterOutcome::ChangedOrInactive);
        assert_eq!(state.event_checks, 0);
        assert!(state.persisted.is_empty());
        assert_eq!(state.checkpoints, 0);
        assert_eq!(state.commits, 0);
        assert_eq!(attempt_report.matches_inserted, 0);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_checkpoint_when_accepted_work_is_retryable()
    -> Result<(), RunPeriodicSearchFilterMatchingError> {
        let state = state(PeriodicSearchFilterProgressLockOutcome::Current {
            matched_through: OffsetDateTime::UNIX_EPOCH,
        });
        let handler = handler(Arc::clone(&state))?;
        let filter = filter();
        let accepted = accepted_match(&filter);
        let mut attempt_report = FilterAttemptReport::default();

        let outcome = handler
            .commit_filter(
                &filter,
                OffsetDateTime::UNIX_EPOCH + Duration::seconds(1),
                vec![accepted.clone()],
                false,
                &mut attempt_report,
            )
            .await?;

        let state = state
            .lock()
            .map_err(|_| RunPeriodicSearchFilterMatchingError::InvalidPolicy)?;
        assert_eq!(outcome, FilterOutcome::Completed);
        assert_eq!(state.event_checks, 1);
        assert_eq!(state.persisted, vec![accepted]);
        assert_eq!(state.checkpoints, 0);
        assert_eq!(state.commits, 1);
        assert_eq!(attempt_report.matches_inserted, 1);
        Ok(())
    }

    #[tokio::test]
    async fn should_skip_stale_current_product_listing_match_and_advance_progress()
    -> Result<(), RunPeriodicSearchFilterMatchingError> {
        let state = state(PeriodicSearchFilterProgressLockOutcome::Current {
            matched_through: OffsetDateTime::UNIX_EPOCH,
        });
        state
            .lock()
            .map_err(|_| RunPeriodicSearchFilterMatchingError::InvalidPolicy)?
            .current_event_check = ProductListingCurrentEventCheck::Stale;
        let handler = handler(Arc::clone(&state))?;
        let filter = filter();
        let mut attempt_report = FilterAttemptReport::default();

        let outcome = handler
            .commit_filter(
                &filter,
                OffsetDateTime::UNIX_EPOCH + Duration::seconds(1),
                vec![accepted_match(&filter)],
                true,
                &mut attempt_report,
            )
            .await?;

        let state = state
            .lock()
            .map_err(|_| RunPeriodicSearchFilterMatchingError::InvalidPolicy)?;
        assert_eq!(FilterOutcome::Completed, outcome);
        assert_eq!(1, state.event_checks);
        assert!(state.persisted.is_empty());
        assert_eq!(1, state.checkpoints);
        assert_eq!(1, state.commits);
        assert_eq!(1, attempt_report.candidates_stale);
        assert_eq!(0, attempt_report.matches_inserted);
        assert_eq!(0, attempt_report.matches_duplicate);
        Ok(())
    }

    #[tokio::test]
    async fn should_persist_accepted_work_only_after_current_filter_revalidation()
    -> Result<(), RunPeriodicSearchFilterMatchingError> {
        let state = state(PeriodicSearchFilterProgressLockOutcome::Current {
            matched_through: OffsetDateTime::UNIX_EPOCH,
        });
        let handler = handler(Arc::clone(&state))?;
        let filter = filter();
        let accepted = accepted_match(&filter);
        let mut attempt_report = FilterAttemptReport::default();

        let outcome = handler
            .commit_filter(
                &filter,
                OffsetDateTime::UNIX_EPOCH + Duration::seconds(1),
                vec![accepted.clone()],
                true,
                &mut attempt_report,
            )
            .await?;

        let state = state
            .lock()
            .map_err(|_| RunPeriodicSearchFilterMatchingError::InvalidPolicy)?;
        assert_eq!(outcome, FilterOutcome::Completed);
        assert_eq!(state.event_checks, 1);
        assert_eq!(state.persisted, vec![accepted]);
        assert_eq!(state.checkpoints, 1);
        assert_eq!(state.commits, 1);
        assert_eq!(attempt_report.matches_inserted, 1);
        Ok(())
    }
    fn seed_candidates(
        state: &Arc<Mutex<State>>,
        count: usize,
    ) -> Vec<ProductListingSearchFilterMatchSourceRef> {
        let mut state = state.lock().unwrap();
        let mut result = ProductListingSearchReadResult::default();
        let mut refs = Vec::new();
        for index in 0..count {
            let mut source = crate::product_match_evaluator::tests::product().unwrap();
            source
                .titles
                .insert(Language::En, format!("Candidate {index}").into());
            let reference = ProductListingSearchFilterMatchSourceRef {
                product_listing_id: source.product_listing_id,
                event_id: source.event_id,
            };
            result.items.push(ProductListingSearchItem {
                geography: Default::default(),
                product_listing_id: reference.product_listing_id,
                product_listing_title_slug_id: source.product_listing_title_slug_id.clone(),
                event_id: reference.event_id,
                listing_source_id: source.source.listing_source_id,
                source_listing_id: source.source_listing_id.clone(),
                auction_id: None,
                title: None,
                display_price: None,
                price_valuation: ProductListingSummaryPriceValuation::Current {
                    fx_rate_id: FxRateId::new(),
                    captured_at: OffsetDateTime::UNIX_EPOCH,
                },
                availability: source.availability,
                lifecycle: source.lifecycle,
                url: source.url.clone(),
                images: source.images.clone(),
                updated: source.updated,
            });
            state.sources.insert(reference, source);
            refs.push(reference);
        }
        state.product_listing_search_result = Some(result);
        refs
    }

    fn decision(accepted: bool) -> Result<ClassificationResponse, ClassificationError> {
        Ok(ClassificationResponse {
            answers: std::collections::BTreeMap::from([(
                classifier_model::QuestionId::new("should_show").unwrap(),
                Probability::new(if accepted { 0.75 } else { 0.1 }).unwrap(),
            )]),
            usage: Default::default(),
            diagnostics: Default::default(),
        })
    }

    fn full_budget_handler(state: Arc<Mutex<State>>) -> Handler {
        let mut handler = handler(state).unwrap();
        handler.policy.hybrid_scan_limit = NonZeroUsize::new(100).unwrap();
        handler.policy.evaluation_limit = NonZeroUsize::new(50).unwrap();
        handler.policy.classification_concurrency = NonZeroUsize::new(8).unwrap();
        handler
    }

    fn ordinary_filter() -> PeriodicSearchFilterCandidate {
        let mut filter = filter();
        filter
            .search
            .product_listing_query
            .push("antique".try_into().unwrap());
        filter.matching_evidence_policy = search_filter_core::MatchingEvidencePolicy::TextOnly;
        filter
    }

    #[tokio::test]
    async fn all_fifty_are_evaluated_regardless_of_rejection_prefix_and_no_tail_is_chased() {
        for batch_size in [1, 32, 50] {
            for rejected_prefix in [10, 49, 50] {
                let state = state(PeriodicSearchFilterProgressLockOutcome::Current {
                    matched_through: OffsetDateTime::UNIX_EPOCH,
                });
                let refs = seed_candidates(&state, 51);
                state.lock().unwrap().evaluator_results = (0..50)
                    .map(|rank| decision(rank >= rejected_prefix))
                    .collect();
                let mut handler = full_budget_handler(state.clone());
                handler.policy.classification_batch_candidates =
                    NonZeroUsize::new(batch_size).unwrap();
                let mut selected = None;
                let mut report = FilterAttemptReport::default();
                let outcome = handler
                    .process_filter(
                        &ordinary_filter(),
                        &test_snapshot(),
                        OffsetDateTime::UNIX_EPOCH + Duration::hours(1),
                        &mut selected,
                        &mut report,
                    )
                    .await
                    .unwrap();
                assert_eq!(FilterOutcome::Completed, outcome);
                assert_eq!(selected.as_ref().unwrap(), &refs[..50]);
                assert_eq!(51, report.candidates_scanned);
                assert_eq!(50, report.candidates_selected);
                assert_eq!(50, report.classification_requests);
                assert_eq!(rejected_prefix, report.candidates_rejected);
                assert_eq!(50 - rejected_prefix, report.matches_inserted);
                assert_eq!(1, report.filters_candidate_cap);
                let state = state.lock().unwrap();
                assert_eq!(50_usize.div_ceil(batch_size), state.evaluator_calls);
                assert!(
                    state
                        .evaluator_requests
                        .iter()
                        .all(|request| request.image_urls.is_empty())
                );
                assert!(
                    state
                        .persisted
                        .iter()
                        .all(|item| item.product_listing_id != refs[50].product_listing_id)
                );
                if rejected_prefix == 49 {
                    assert_eq!(
                        refs[49].product_listing_id,
                        state.persisted[0].product_listing_id
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn batch_timeout_keeps_frozen_selection_and_retry_does_not_duplicate_matches() {
        for batch_size in [32, 50] {
            let successful = if batch_size == 32 { 32 } else { 0 };
            let state = state(PeriodicSearchFilterProgressLockOutcome::Current {
                matched_through: OffsetDateTime::UNIX_EPOCH,
            });
            let refs = seed_candidates(&state, 51);
            // Exercise a split batch and a timeout of the complete fifty-member batch.
            state.lock().unwrap().evaluator_results = (0..successful + 1)
                .map(|index| {
                    if index == successful {
                        Err(ClassificationError::Timeout)
                    } else {
                        decision(true)
                    }
                })
                .collect();
            let mut handler = full_budget_handler(state.clone());
            handler.policy.classification_batch_candidates = NonZeroUsize::new(batch_size).unwrap();
            let filter = ordinary_filter();
            let window = OffsetDateTime::UNIX_EPOCH + Duration::hours(1);
            let mut selected = None;
            let mut first = FilterAttemptReport::default();
            assert_eq!(
                FilterOutcome::Retryable,
                handler
                    .process_filter(&filter, &test_snapshot(), window, &mut selected, &mut first)
                    .await
                    .unwrap()
            );
            assert_eq!(Some(refs[..50].to_vec()), selected);
            assert_eq!(successful, first.matches_inserted);
            assert_eq!(50 - successful, first.retryable_evaluation_failures);
            assert_eq!(0, first.candidates_rejected);
            assert_eq!(0, state.lock().unwrap().checkpoints);
            state.lock().unwrap().evaluator_results = (0..50).map(|_| decision(true)).collect();
            let mut second = FilterAttemptReport::default();
            assert_eq!(
                FilterOutcome::Completed,
                handler
                    .process_filter(
                        &filter,
                        &test_snapshot(),
                        window,
                        &mut selected,
                        &mut second
                    )
                    .await
                    .unwrap()
            );
            assert_eq!(Some(refs[..50].to_vec()), selected);
            assert_eq!(50 - successful, second.matches_inserted);
            assert_eq!(successful, second.matches_duplicate);
            let state = state.lock().unwrap();
            assert_eq!(50, state.persisted.len());
            assert!(
                state
                    .persisted
                    .iter()
                    .all(|m| m.product_listing_id != refs[50].product_listing_id)
            );
            assert_eq!(1, state.checkpoints);
        }
    }

    #[tokio::test]
    async fn cheap_skips_do_not_spend_slots_and_selected_set_survives_retry() {
        let state = state(PeriodicSearchFilterProgressLockOutcome::Current {
            matched_through: OffsetDateTime::UNIX_EPOCH,
        });
        let refs = seed_candidates(&state, 60);
        {
            let mut state = state.lock().unwrap();
            state.existing.insert(refs[0].product_listing_id);
            state.sources.remove(&refs[1]);
            state.sources.get_mut(&refs[2]).unwrap().current_event_id = EventId::new();
            state.sources.get_mut(&refs[3]).unwrap().lifecycle = ListingLifecycle::Withdrawn;
            let duplicate = state.product_listing_search_result.as_ref().unwrap().items[4].clone();
            state
                .product_listing_search_result
                .as_mut()
                .unwrap()
                .items
                .insert(5, duplicate);
            state.evaluator_results = (0..50)
                .map(|index| {
                    if index == 49 {
                        Err(ClassificationError::Timeout)
                    } else {
                        decision(true)
                    }
                })
                .collect();
        }
        let handler = full_budget_handler(state.clone());
        let filter = ordinary_filter();
        let mut selected = None;
        let mut first = FilterAttemptReport::default();
        let window_end = OffsetDateTime::UNIX_EPOCH + Duration::hours(1);
        assert_eq!(
            FilterOutcome::Retryable,
            handler
                .process_filter(
                    &filter,
                    &test_snapshot(),
                    window_end,
                    &mut selected,
                    &mut first
                )
                .await
                .unwrap()
        );
        assert_eq!(50, first.candidates_selected);
        assert_eq!(1, first.candidates_duplicate);
        assert_eq!(1, first.candidates_existing);
        assert_eq!(1, first.candidates_missing_source);
        assert_eq!(1, first.candidates_stale);
        assert_eq!(1, first.candidates_withdrawn);
        assert_eq!(49, first.matches_inserted);
        assert_eq!(1, first.retryable_evaluation_failures);
        assert_eq!(0, state.lock().unwrap().checkpoints);
        let selected_before = selected.clone();
        state.lock().unwrap().evaluator_results = (0..50).map(|_| decision(false)).collect();
        let mut second = FilterAttemptReport::default();
        assert_eq!(
            FilterOutcome::Completed,
            handler
                .process_filter(
                    &filter,
                    &test_snapshot(),
                    window_end,
                    &mut selected,
                    &mut second
                )
                .await
                .unwrap()
        );
        assert_eq!(selected_before, selected);
        assert_eq!(0, second.candidates_scanned);
        assert_eq!(0, second.candidates_selected);
        assert_eq!(50, second.classification_requests);
        let state = state.lock().unwrap();
        assert_eq!(100, state.evaluator_calls);
        assert_eq!(1, state.checkpoints);
        assert_eq!(
            state.evaluator_requests[..50],
            state.evaluator_requests[50..]
        );
    }

    #[tokio::test]
    async fn deterministic_searches_skip_embedding_and_classifier_with_the_same_budget() {
        for count in [0, 3, 51, 100] {
            let state = state(PeriodicSearchFilterProgressLockOutcome::Current {
                matched_through: OffsetDateTime::UNIX_EPOCH,
            });
            seed_candidates(&state, count);
            let handler = full_budget_handler(state.clone());
            let mut report = FilterAttemptReport::default();
            let outcome = handler
                .process_filter(
                    &filter(),
                    &test_snapshot(),
                    OffsetDateTime::UNIX_EPOCH + Duration::hours(1),
                    &mut None,
                    &mut report,
                )
                .await
                .unwrap();
            assert_eq!(FilterOutcome::Completed, outcome);
            assert_eq!(count.min(50), report.candidates_selected);
            assert_eq!(count.min(50), report.matches_inserted);
            assert_eq!(0, report.classification_requests);
            assert_eq!(usize::from(count == 100), report.filters_scan_cap);
            assert_eq!(0, state.lock().unwrap().evaluator_calls);
        }
    }

    #[test]
    fn enhanced_retrieval_keeps_complete_intent_out_of_the_mandatory_title_query() {
        let mut filter = ordinary_filter();
        let enhanced =
            "Prefer blue tones, but include red objects too; exclude reproductions. ".repeat(8);
        filter.search.enhanced_search_description = Some(enhanced.try_into().unwrap());
        let search = periodic_search(
            &filter,
            OffsetDateTime::UNIX_EPOCH + Duration::hours(1),
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(
            filter.search.product_listing_query,
            search.product_listing_query
        );
        assert_eq!(
            filter.search.enhanced_search_description,
            search.enhanced_search_description
        );
        assert!(has_semantic_intent(&search));
        assert!(!has_semantic_intent(&ProductListingSearch::default()));
    }

    #[tokio::test]
    async fn policy_change_during_inference_fences_all_accepted_results_and_progress() {
        let state = state(PeriodicSearchFilterProgressLockOutcome::Current {
            matched_through: OffsetDateTime::UNIX_EPOCH,
        });
        seed_candidates(&state, 3);
        {
            let mut state = state.lock().unwrap();
            state.policy_change_during_evaluation = true;
            state.evaluator_results = (0..3).map(|_| decision(true)).collect();
        }
        let handler = full_budget_handler(state.clone());
        let mut report = FilterAttemptReport::default();
        let outcome = handler
            .process_filter(
                &ordinary_filter(),
                &test_snapshot(),
                OffsetDateTime::UNIX_EPOCH + Duration::hours(1),
                &mut None,
                &mut report,
            )
            .await
            .unwrap();
        assert_eq!(FilterOutcome::ChangedOrInactive, outcome);
        assert_eq!(3, report.classification_requests);
        assert_eq!(0, report.matches_inserted);
        assert_eq!(0, state.lock().unwrap().checkpoints);
        assert!(state.lock().unwrap().persisted.is_empty());
    }

    #[tokio::test]
    async fn semantic_zero_and_small_pools_report_exhaustion_without_fabricating_decisions() {
        for count in [0, 3] {
            let state = state(PeriodicSearchFilterProgressLockOutcome::Current {
                matched_through: OffsetDateTime::UNIX_EPOCH,
            });
            seed_candidates(&state, count);
            state.lock().unwrap().evaluator_results = (0..count).map(|_| decision(false)).collect();
            let handler = full_budget_handler(state.clone());
            let mut report = FilterAttemptReport::default();
            assert_eq!(
                FilterOutcome::Completed,
                handler
                    .process_filter(
                        &ordinary_filter(),
                        &test_snapshot(),
                        OffsetDateTime::UNIX_EPOCH + Duration::hours(1),
                        &mut None,
                        &mut report
                    )
                    .await
                    .unwrap()
            );
            assert_eq!(count, report.candidates_selected);
            assert_eq!(count, report.classification_requests);
            assert_eq!(count, report.candidates_rejected);
            assert_eq!(0, report.filters_candidate_cap);
            assert_eq!(0, report.filters_scan_cap);
            assert_eq!(1, state.lock().unwrap().checkpoints);
        }
    }

    #[tokio::test]
    async fn terminal_candidate_failures_are_distinct_from_rejections_and_retryable_work() {
        let state = state(PeriodicSearchFilterProgressLockOutcome::Current {
            matched_through: OffsetDateTime::UNIX_EPOCH,
        });
        seed_candidates(&state, 3);
        state.lock().unwrap().evaluator_results = [
            decision(true),
            Err(ClassificationError::PermanentCandidateFailure),
            decision(false),
        ]
        .into();
        let handler = full_budget_handler(state.clone());
        let mut report = FilterAttemptReport::default();
        assert_eq!(
            FilterOutcome::Completed,
            handler
                .process_filter(
                    &ordinary_filter(),
                    &test_snapshot(),
                    OffsetDateTime::UNIX_EPOCH + Duration::hours(1),
                    &mut None,
                    &mut report,
                )
                .await
                .unwrap()
        );
        assert_eq!(3, report.classification_requests);
        assert_eq!(1, report.candidates_rejected);
        assert_eq!(1, report.permanent_evaluation_failures);
        assert_eq!(0, report.retryable_evaluation_failures);
        assert_eq!(1, report.matches_inserted);
        assert_eq!(1, state.lock().unwrap().checkpoints);
    }

    #[test]
    fn malformed_persisted_semantic_embeddings_fail_closed_without_provider_work() {
        for values in [
            vec![],
            vec![0.1],
            vec![f32::NAN; embedding::EMBEDDING_DIMENSIONS],
            vec![f32::INFINITY; embedding::EMBEDDING_DIMENSIONS],
            vec![0.0; embedding::EMBEDDING_DIMENSIONS],
        ] {
            let mut filter = filter();
            filter.embedding = Some(values);
            assert_eq!(
                Err(FilterOutcome::InvalidPersistedState),
                filter_embedding(&filter)
            );
        }
    }

    #[test]
    fn should_preserve_product_search_failure_sources_when_mapping_reader_errors() {
        let source_message = |error: &RunPeriodicSearchFilterMatchingError| {
            std::error::Error::source(error).map(ToString::to_string)
        };

        let query_failed = product_search_error(
            ProductListingSearchReadError::ProductListingSearchQueryFailed {
                source: box_error(std::io::Error::other("connection refused")),
            },
        );
        assert!(matches!(
            query_failed,
            RunPeriodicSearchFilterMatchingError::ProductListingSearchFailed { .. }
        ));
        assert_eq!(FilterRetryClass::Retryable, query_failed.retry_class());
        assert_eq!(
            Some("connection refused".to_owned()),
            source_message(&query_failed)
        );

        let read_model_invalid = product_search_error(
            ProductListingSearchReadError::ProductListingSearchReadModelInvalid {
                source: box_error(std::io::Error::other("invalid search payload")),
            },
        );
        assert!(matches!(
            read_model_invalid,
            RunPeriodicSearchFilterMatchingError::ProductListingSearchResultInvalid { .. }
        ));
        assert_eq!(
            FilterRetryClass::Permanent,
            read_model_invalid.retry_class()
        );
        assert_eq!(
            Some("invalid search payload".to_owned()),
            source_message(&read_model_invalid)
        );

        let invalid_cursor = product_search_error(ProductListingSearchReadError::InvalidCursor);
        assert!(matches!(
            invalid_cursor,
            RunPeriodicSearchFilterMatchingError::ProductListingSearchResultInvalid { .. }
        ));
        assert_eq!(
            Some("invalid product search continuation cursor".to_owned()),
            source_message(&invalid_cursor)
        );
    }
}
