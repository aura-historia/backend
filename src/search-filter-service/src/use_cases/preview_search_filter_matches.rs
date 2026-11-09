//! A preview of the listings a search filter's criteria match now, decided exactly as
//! scheduled matching decides them, but over the whole catalogue instead of recent changes.
//!
//! Each candidate is classified on its own: the singleton decision is scheduled matching's
//! default and the reference its optional shared requests are measured against.

use crate::match_candidates::{
    CandidateEligibility, candidate_eligibility, has_semantic_intent, retrieve_candidates,
    retrieves_with_embedding,
};
use crate::product_match_evaluator::{
    ProductListingMatchEvaluationOutcome, ProductListingMatchEvaluationRequest,
    evaluate_product_matches,
};
use crate::tier_policy::{matching_evidence_policy, validate_search_features};
use crate::use_cases::embedding_query;
use application::error::{BoxError, box_error};
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use application::transaction::{Transaction, UnitOfWork};
use classifier_model::{ClassificationError, ClassifierModel, Probability};
use embedding::{EmbeddingError, EmbeddingGenerator};
use futures::StreamExt;
use fxrate_core::FxRateSnapshot;
use fxrate_service::ports::{FxRateSnapshotReadError, FxRateSnapshotReader};
use product_listing_core::product_listing_search::ProductListingSearch;
use product_listing_service::ports::{
    CompiledProductListingSearch, ProductListingPriceFilterPlan,
    ProductListingSearchFilterMatchSource, ProductListingSearchFilterMatchSourceReadError,
    ProductListingSearchFilterMatchSourceReader,
    ProductListingSearchFilterMatchSourceReaderFactory, ProductListingSearchFilterMatchSourceRef,
    ProductListingSearchReadError, ProductListingSearchReader,
};
use product_listing_service::use_cases::{
    PersonalizedProductListingSummary, ProductListingSearchItem, ProductListingSearchPresenter,
    SearchProductListingsError,
};
use search_filter_core::MatchingEvidencePolicy;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::time::Duration;
use time::OffsetDateTime;
use user_core::tier::UserTier;
use user_core::user_id::UserId;
use user_service::ports::{
    UserTierEntitlements, UserTierEntitlementsError, UserTierEntitlementsFactory,
};

/// The most listings one preview returns.
pub const SEARCH_FILTER_MATCH_PREVIEW_LIMIT: usize = 4;
/// Classification of one preview gives up after this long and fails as unavailable.
pub const SEARCH_FILTER_MATCH_PREVIEW_CLASSIFICATION_DEADLINE: Duration = Duration::from_secs(5);
/// Interactive bound on fetching one candidate image; a slower image is omitted, as any failed
/// fetch is, instead of using up the classification deadline.
pub const SEARCH_FILTER_MATCH_PREVIEW_IMAGE_FETCH_DEADLINE: Duration = Duration::from_secs(1);
/// The most candidates one preview sends to the classifier.
const PREVIEW_EVALUATION_LIMIT: usize = 16;
/// Candidates classified at once; results are still consumed in rank order.
const PREVIEW_CLASSIFICATION_CONCURRENCY: usize = SEARCH_FILTER_MATCH_PREVIEW_LIMIT;
/// Waits before retrying an undecided candidate, as scheduled matching retries a filter.
const PREVIEW_RETRY_DELAYS: [Duration; 2] = [Duration::from_millis(250), Duration::from_secs(1)];
/// Retrieval headroom so skipped candidates rarely leave the evaluation budget unused.
const PREVIEW_SCAN_LIMIT: NonZeroUsize = NonZeroUsize::new(32).unwrap();

#[derive(Debug, Clone, PartialEq)]
pub struct PreviewSearchFilterMatchesCommand {
    pub user_id: UserId,
    pub search: ProductListingSearch,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchFilterMatchPreview {
    /// At most [`SEARCH_FILTER_MATCH_PREVIEW_LIMIT`] matches, most relevant first.
    pub items: Vec<PersonalizedProductListingSummary>,
    /// Fewer matches than the limit were found before the preview stopped looking, so more
    /// listings may match than it shows.
    pub candidate_limit_reached: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchFilterMatchPreviewPolicy {
    pub should_show_threshold: Probability,
    pub classification_deadline: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum PreviewSearchFilterMatchesError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("actor may not manage this search filter")]
    ActorMayNotManageSearchFilter,
    #[error("user not found")]
    UserNotFound,
    #[error("search filter feature '{feature}' requires a higher user tier")]
    SearchFilterFeatureRestricted { feature: &'static str },
    #[error("user tier read failed")]
    UserTierReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter embedding generation failed")]
    EmbeddingGenerationFailed {
        #[source]
        source: BoxError,
    },
    #[error("FX rate snapshot read failed")]
    FxRateSnapshotReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("FX rate snapshot is invalid")]
    FxRateSnapshotInvalid {
        #[source]
        source: BoxError,
    },
    #[error("no FX rate snapshot is available")]
    FxRateSnapshotMissing,
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
    #[error("classifier could not decide a candidate right now")]
    ClassifierUnavailable {
        #[source]
        source: ClassificationError,
    },
    #[error("classifier rejected the request; its configuration needs operator action")]
    ClassifierMisconfigured {
        #[source]
        source: ClassificationError,
    },
    #[error("matched listings could not be presented")]
    Presentation(#[source] SearchProductListingsError),
    #[error("failed to begin search filter preview transaction")]
    BeginTransactionFailed(#[source] BoxError),
    #[error("failed to commit search filter preview transaction")]
    CommitTransactionFailed(#[source] BoxError),
}

impl PreviewSearchFilterMatchesError {
    /// Rejections of the caller's request, as opposed to failures the service must report.
    fn is_caller_error(&self) -> bool {
        matches!(
            self,
            Self::AuthenticatedActorRequired
                | Self::ActorMayNotManageSearchFilter
                | Self::UserNotFound
                | Self::SearchFilterFeatureRestricted { .. }
        )
    }
}

#[async_trait::async_trait]
pub trait PreviewSearchFilterMatchesUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: PreviewSearchFilterMatchesCommand,
    ) -> Result<SearchFilterMatchPreview, PreviewSearchFilterMatchesError>;
}

pub struct PreviewSearchFilterMatchesHandler<U, T, E, F, P, S, C, R> {
    unit_of_work: U,
    tier_entitlements: T,
    embeddings: E,
    fx_rates: F,
    product_listings: P,
    sources: S,
    classifier: C,
    presenter: R,
    policy: SearchFilterMatchPreviewPolicy,
}

impl<U, T, E, F, P, S, C, R> PreviewSearchFilterMatchesHandler<U, T, E, F, P, S, C, R> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        unit_of_work: U,
        tier_entitlements: T,
        embeddings: E,
        fx_rates: F,
        product_listings: P,
        sources: S,
        classifier: C,
        presenter: R,
        policy: SearchFilterMatchPreviewPolicy,
    ) -> Self {
        Self {
            unit_of_work,
            tier_entitlements,
            embeddings,
            fx_rates,
            product_listings,
            sources,
            classifier,
            presenter,
            policy,
        }
    }
}

/// A retrieved candidate whose current source may be classified.
struct EligibleCandidate<'a> {
    item: ProductListingSearchItem,
    source: &'a ProductListingSearchFilterMatchSource,
}

struct EligibleCandidates<'a> {
    candidates: Vec<EligibleCandidate<'a>>,
    /// Eligible candidates beyond the evaluation budget, or beyond the retrieval window.
    more_available: bool,
}

#[async_trait::async_trait]
impl<U, T, E, F, P, S, C, R> PreviewSearchFilterMatchesUseCase
    for PreviewSearchFilterMatchesHandler<U, T, E, F, P, S, C, R>
where
    U: UnitOfWork,
    T: UserTierEntitlementsFactory<U::Tx>,
    E: EmbeddingGenerator,
    F: FxRateSnapshotReader,
    P: ProductListingSearchReader,
    S: ProductListingSearchFilterMatchSourceReaderFactory<U::Tx>,
    C: ClassifierModel,
    R: ProductListingSearchPresenter,
{
    #[tracing::instrument(
        name = "preview_search_filter_matches",
        skip_all,
        fields(
            user_id = %command.user_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: PreviewSearchFilterMatchesCommand,
    ) -> Result<SearchFilterMatchPreview, PreviewSearchFilterMatchesError> {
        let result = self.preview(context, command).await;
        if let Err(error) = &result
            && !error.is_caller_error()
        {
            tracing::warn!(
                error = %error,
                cause = std::error::Error::source(error).map(tracing::field::display),
                "search_filter.preview.failed"
            );
        }
        result
    }
}

impl<U, T, E, F, P, S, C, R> PreviewSearchFilterMatchesHandler<U, T, E, F, P, S, C, R>
where
    U: UnitOfWork,
    T: UserTierEntitlementsFactory<U::Tx>,
    E: EmbeddingGenerator,
    F: FxRateSnapshotReader,
    P: ProductListingSearchReader,
    S: ProductListingSearchFilterMatchSourceReaderFactory<U::Tx>,
    C: ClassifierModel,
    R: ProductListingSearchPresenter,
{
    async fn preview(
        &self,
        context: &OperationContext,
        command: PreviewSearchFilterMatchesCommand,
    ) -> Result<SearchFilterMatchPreview, PreviewSearchFilterMatchesError> {
        authorize_owner(context, command.user_id)?;
        let tier = self.user_tier(command.user_id).await?;
        validate_search_features(tier, &command.search).map_err(|feature| {
            PreviewSearchFilterMatchesError::SearchFilterFeatureRestricted { feature }
        })?;
        let evidence_policy = matching_evidence_policy(tier);
        let embedding = self.retrieval_embedding(&command.search).await?;
        let snapshot = self.current_snapshot().await?;
        let compiled_search = compile_search(command.search.clone(), snapshot.clone())?;
        let retrieved = retrieve_candidates(
            &self.product_listings,
            compiled_search,
            embedding.as_deref(),
            PREVIEW_SCAN_LIMIT,
        )
        .await
        .map_err(product_search_error)?;
        let scan_limit_reached = retrieved.scan_limit_reached;
        let refs = retrieved
            .items
            .iter()
            .map(|item| ProductListingSearchFilterMatchSourceRef {
                product_listing_id: item.product_listing_id,
                event_id: item.event_id,
            })
            .collect::<Vec<_>>();
        let sources = self.load_sources(&refs).await?;
        let eligible = select_eligible(retrieved.items, &sources, scan_limit_reached)?;
        let evaluated = eligible.candidates.len();
        let more_available = eligible.more_available;
        let matches = self
            .accept_in_rank_order(
                context,
                &command.search,
                evidence_policy,
                eligible.candidates,
            )
            .await?;
        let candidate_limit_reached =
            matches.len() < SEARCH_FILTER_MATCH_PREVIEW_LIMIT && more_available;
        tracing::info!(
            evidence_policy = evidence_policy.as_str(),
            classified = has_semantic_intent(&command.search),
            retrieved = refs.len(),
            eligible = evaluated,
            matched = matches.len(),
            candidate_limit_reached,
            "search_filter.preview.completed"
        );
        let items = self
            .presenter
            .present(context, matches, &command.search, &snapshot)
            .await
            .map_err(PreviewSearchFilterMatchesError::Presentation)?;
        Ok(SearchFilterMatchPreview {
            items,
            candidate_limit_reached,
        })
    }
}

impl<U, T, E, F, P, S, C, R> PreviewSearchFilterMatchesHandler<U, T, E, F, P, S, C, R>
where
    U: UnitOfWork,
    T: UserTierEntitlementsFactory<U::Tx>,
    E: EmbeddingGenerator,
    F: FxRateSnapshotReader,
    S: ProductListingSearchFilterMatchSourceReaderFactory<U::Tx>,
    C: ClassifierModel,
{
    async fn user_tier(
        &self,
        user_id: UserId,
    ) -> Result<UserTier, PreviewSearchFilterMatchesError> {
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            PreviewSearchFilterMatchesError::BeginTransactionFailed(box_error(source))
        })?;
        let tier = self
            .tier_entitlements
            .in_transaction(&mut tx)
            .lock_user_tier(user_id)
            .await
            .map_err(tier_entitlements_error)?
            .ok_or(PreviewSearchFilterMatchesError::UserNotFound)?;
        tx.commit().await.map_err(|source| {
            PreviewSearchFilterMatchesError::CommitTransactionFailed(box_error(source))
        })?;
        Ok(tier)
    }

    /// Scheduled matching retrieves enhanced intent with the embedding stored on the filter;
    /// the preview derives the same embedding from the same text.
    async fn retrieval_embedding(
        &self,
        search: &ProductListingSearch,
    ) -> Result<Option<Vec<f32>>, PreviewSearchFilterMatchesError> {
        if !retrieves_with_embedding(search) {
            return Ok(None);
        }
        let Some(query) = embedding_query(search).map_err(embedding_error)? else {
            return Ok(None);
        };
        let embedding = self
            .embeddings
            .embed_search_query(&query)
            .await
            .map_err(embedding_error)?;
        Ok(Some(embedding.into_values()))
    }

    async fn current_snapshot(&self) -> Result<FxRateSnapshot, PreviewSearchFilterMatchesError> {
        self.fx_rates
            .find_latest_at_or_before(OffsetDateTime::now_utc())
            .await
            .map_err(fx_rate_snapshot_read_error)?
            .ok_or(PreviewSearchFilterMatchesError::FxRateSnapshotMissing)
    }

    async fn load_sources(
        &self,
        refs: &[ProductListingSearchFilterMatchSourceRef],
    ) -> Result<
        HashMap<ProductListingSearchFilterMatchSourceRef, ProductListingSearchFilterMatchSource>,
        PreviewSearchFilterMatchesError,
    > {
        if refs.is_empty() {
            return Ok(HashMap::new());
        }
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            PreviewSearchFilterMatchesError::BeginTransactionFailed(box_error(source))
        })?;
        let sources = self
            .sources
            .in_transaction(&mut tx)
            .find_sources(refs)
            .await
            .map_err(product_source_error)?;
        tx.commit().await.map_err(|source| {
            PreviewSearchFilterMatchesError::CommitTransactionFailed(box_error(source))
        })?;
        Ok(sources)
    }

    /// Classifies candidates in rank order, a few at a time, and stops once the preview is
    /// full. A candidate whose decision stays unknown fails the preview, because the matches
    /// shown must be the ones scheduled matching would accept.
    async fn accept_in_rank_order(
        &self,
        context: &OperationContext,
        search: &ProductListingSearch,
        evidence_policy: MatchingEvidencePolicy,
        candidates: Vec<EligibleCandidate<'_>>,
    ) -> Result<Vec<ProductListingSearchItem>, PreviewSearchFilterMatchesError> {
        if !has_semantic_intent(search) {
            return Ok(candidates
                .into_iter()
                .take(SEARCH_FILTER_MATCH_PREVIEW_LIMIT)
                .map(|candidate| candidate.item)
                .collect());
        }
        let accepted = tokio::time::timeout(
            self.policy.classification_deadline,
            self.classify_until_full(context, search, evidence_policy, &candidates),
        )
        .await
        .map_err(|_| PreviewSearchFilterMatchesError::ClassifierUnavailable {
            source: ClassificationError::Timeout,
        })??;
        let mut candidates = candidates.into_iter().map(Some).collect::<Vec<_>>();
        Ok(accepted
            .into_iter()
            .filter_map(|index| candidates[index].take())
            .map(|candidate| candidate.item)
            .collect())
    }

    /// Indexes of the first accepted candidates, in rank order. A few candidates are classified
    /// at a time but consumed in rank order; once the preview is full, classifications still in
    /// flight are dropped.
    async fn classify_until_full(
        &self,
        context: &OperationContext,
        search: &ProductListingSearch,
        evidence_policy: MatchingEvidencePolicy,
        candidates: &[EligibleCandidate<'_>],
    ) -> Result<Vec<usize>, PreviewSearchFilterMatchesError> {
        let decisions = candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| {
                let decision = self.decide(context, search, evidence_policy, candidate);
                async move { decision.await.map(|accepted| accepted.then_some(index)) }
            })
            .collect::<Vec<_>>();
        let mut decisions =
            futures::stream::iter(decisions).buffered(PREVIEW_CLASSIFICATION_CONCURRENCY);
        let mut accepted = Vec::new();
        while let Some(decision) = decisions.next().await {
            if let Some(index) = decision? {
                accepted.push(index);
            }
            if accepted.len() == SEARCH_FILTER_MATCH_PREVIEW_LIMIT {
                break;
            }
        }
        Ok(accepted)
    }

    /// Whether scheduled matching would accept `candidate`. An undecided candidate is retried
    /// with scheduled matching's filter backoff; a classifier configuration failure is not.
    async fn decide(
        &self,
        context: &OperationContext,
        search: &ProductListingSearch,
        evidence_policy: MatchingEvidencePolicy,
        candidate: &EligibleCandidate<'_>,
    ) -> Result<bool, PreviewSearchFilterMatchesError> {
        let mut retry_delays = PREVIEW_RETRY_DELAYS.iter();
        loop {
            let request = ProductListingMatchEvaluationRequest {
                key: (),
                product: candidate.source,
                search,
                evidence_policy,
                correlation_id: Some(format!(
                    "preview:{}:{}:{}",
                    context.request_id, candidate.item.product_listing_id, candidate.item.event_id
                )),
            };
            let outcome = evaluate_product_matches(
                &self.classifier,
                vec![request],
                NonZeroUsize::MIN,
                NonZeroUsize::MIN,
                self.policy.should_show_threshold,
            )
            .await
            .pop()
            .map(|result| result.outcome);
            let source = match outcome {
                Some(ProductListingMatchEvaluationOutcome::Matched) => return Ok(true),
                Some(
                    ProductListingMatchEvaluationOutcome::Rejected
                    | ProductListingMatchEvaluationOutcome::PermanentFailure(_),
                ) => return Ok(false),
                Some(ProductListingMatchEvaluationOutcome::RetryableFailure(source)) => source,
                None => ClassificationError::InvalidResponse,
            };
            let error = undecided_candidate_error(source);
            match (&error, retry_delays.next()) {
                (PreviewSearchFilterMatchesError::ClassifierUnavailable { .. }, Some(delay)) => {
                    tokio::time::sleep(*delay).await;
                }
                _ => return Err(error),
            }
        }
    }
}

/// The first eligible candidates in rank order, at most the evaluation budget.
fn select_eligible<'a>(
    items: Vec<ProductListingSearchItem>,
    sources: &'a HashMap<
        ProductListingSearchFilterMatchSourceRef,
        ProductListingSearchFilterMatchSource,
    >,
    scan_limit_reached: bool,
) -> Result<EligibleCandidates<'a>, PreviewSearchFilterMatchesError> {
    let mut candidates = Vec::new();
    for item in items {
        let reference = ProductListingSearchFilterMatchSourceRef {
            product_listing_id: item.product_listing_id,
            event_id: item.event_id,
        };
        match candidate_eligibility(&reference, sources)
            .map_err(|_| PreviewSearchFilterMatchesError::ProductListingSourceMismatch)?
        {
            CandidateEligibility::Eligible(source) => {
                if candidates.len() == PREVIEW_EVALUATION_LIMIT {
                    return Ok(EligibleCandidates {
                        candidates,
                        more_available: true,
                    });
                }
                candidates.push(EligibleCandidate { item, source });
            }
            CandidateEligibility::MissingSource
            | CandidateEligibility::Stale
            | CandidateEligibility::Withdrawn => {}
        }
    }
    Ok(EligibleCandidates {
        candidates,
        more_available: scan_limit_reached,
    })
}

fn compile_search(
    search: ProductListingSearch,
    snapshot: FxRateSnapshot,
) -> Result<CompiledProductListingSearch, PreviewSearchFilterMatchesError> {
    let price_filter_plan =
        ProductListingPriceFilterPlan::compile(snapshot, search.currency, search.price_query)
            .map_err(
                |source| PreviewSearchFilterMatchesError::FxRateSnapshotInvalid {
                    source: box_error(source),
                },
            )?;
    CompiledProductListingSearch::new(search, price_filter_plan).map_err(|source| {
        PreviewSearchFilterMatchesError::FxRateSnapshotInvalid {
            source: box_error(source),
        }
    })
}

fn authorize_owner(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), PreviewSearchFilterMatchesError> {
    context
        .require()
        .credential_capability(CredentialCapability::SearchFiltersWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<PreviewSearchFilterMatchesError>()
}

impl From<OperationAuthorizationError> for PreviewSearchFilterMatchesError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                Self::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => {
                Self::ActorMayNotManageSearchFilter
            }
        }
    }
}

/// A candidate stays undecided: transient provider failures may succeed on retry, while
/// configuration and authorization failures need operator action.
fn undecided_candidate_error(source: ClassificationError) -> PreviewSearchFilterMatchesError {
    match source {
        ClassificationError::Timeout
        | ClassificationError::RateLimited { .. }
        | ClassificationError::Transient
        | ClassificationError::InvalidResponse
        | ClassificationError::PermanentCandidateFailure => {
            PreviewSearchFilterMatchesError::ClassifierUnavailable { source }
        }
        ClassificationError::InvalidRequest
        | ClassificationError::InvalidConfiguration
        | ClassificationError::Authentication
        | ClassificationError::UnsupportedCapability => {
            PreviewSearchFilterMatchesError::ClassifierMisconfigured { source }
        }
    }
}

fn tier_entitlements_error(error: UserTierEntitlementsError) -> PreviewSearchFilterMatchesError {
    match error {
        UserTierEntitlementsError::LockFailed { source }
        | UserTierEntitlementsError::ReconciliationFailed { source } => {
            PreviewSearchFilterMatchesError::UserTierReadFailed { source }
        }
    }
}

fn embedding_error(error: EmbeddingError) -> PreviewSearchFilterMatchesError {
    PreviewSearchFilterMatchesError::EmbeddingGenerationFailed {
        source: box_error(error),
    }
}

fn fx_rate_snapshot_read_error(error: FxRateSnapshotReadError) -> PreviewSearchFilterMatchesError {
    match error {
        FxRateSnapshotReadError::ReadFailed { source } => {
            PreviewSearchFilterMatchesError::FxRateSnapshotReadFailed { source }
        }
        FxRateSnapshotReadError::InvalidPersistedSnapshot { source } => {
            PreviewSearchFilterMatchesError::FxRateSnapshotInvalid { source }
        }
    }
}

fn product_search_error(error: ProductListingSearchReadError) -> PreviewSearchFilterMatchesError {
    match error {
        ProductListingSearchReadError::ProductListingSearchQueryFailed { source } => {
            PreviewSearchFilterMatchesError::ProductListingSearchFailed { source }
        }
        ProductListingSearchReadError::ProductListingSearchReadModelInvalid { source } => {
            PreviewSearchFilterMatchesError::ProductListingSearchResultInvalid { source }
        }
        error @ ProductListingSearchReadError::InvalidCursor => {
            PreviewSearchFilterMatchesError::ProductListingSearchResultInvalid {
                source: box_error(error),
            }
        }
    }
}

fn product_source_error(
    error: ProductListingSearchFilterMatchSourceReadError,
) -> PreviewSearchFilterMatchesError {
    match error {
        ProductListingSearchFilterMatchSourceReadError::InvalidPersistedState { source } => {
            PreviewSearchFilterMatchesError::ProductListingSourceStateInvalid { source }
        }
        error @ ProductListingSearchFilterMatchSourceReadError::QueryFailed { .. } => {
            PreviewSearchFilterMatchesError::ProductListingSourceReadFailed {
                source: box_error(error),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::operation_context::{CorrelationId, Principal, RequestId};
    use application::transaction::TransactionError;
    use classifier_model::{
        ClassificationRequest, ClassificationResponse, Probability, QuestionId,
    };
    use embedding::{EmbeddingImageUrl, EmbeddingText, EmbeddingVector};
    use fxrate_core::{
        FX_RATE_SCALE, FxRateGeneration, FxRateId, FxRateQuote, FxRateSource, NewFxRateSnapshot,
    };
    use localization::Language;
    use money::Currency;
    use product_listing_core::listing_lifecycle::ListingLifecycle;
    use product_listing_core::product_listing_id::ProductListingId;
    use product_listing_core::shipping_search::ShippingSearch;
    use product_listing_service::ports::{
        ProductListingSearchMode, ProductListingSearchReadRequest,
    };
    use product_listing_service::use_cases::{
        ProductListingSearchReadResult, ProductListingSummaryPriceValuation,
    };
    use std::collections::{BTreeSet, HashSet};
    use std::sync::{Arc, Mutex};
    use strum::IntoEnumIterator;

    #[derive(Clone)]
    enum Decision {
        Accept,
        Reject,
        Fail(ClassificationError),
    }

    struct RecordedSearch {
        search: ProductListingSearch,
        size: Option<u64>,
        hybrid: bool,
    }

    struct World {
        owner: UserId,
        tier: UserTier,
        result: ProductListingSearchReadResult,
        sources: HashMap<
            ProductListingSearchFilterMatchSourceRef,
            ProductListingSearchFilterMatchSource,
        >,
        /// Decisions per candidate title, one per call; the last repeats.
        decisions: HashMap<String, Vec<Decision>>,
        delays: HashMap<String, Duration>,
        classified: Vec<ClassificationRequest>,
        searches: Vec<RecordedSearch>,
        embedded: Vec<String>,
        presented: Option<Vec<ProductListingId>>,
    }

    type Shared = Arc<Mutex<World>>;

    fn world(tier: UserTier) -> Shared {
        Arc::new(Mutex::new(World {
            owner: UserId::new(),
            tier,
            result: ProductListingSearchReadResult::default(),
            sources: HashMap::new(),
            decisions: HashMap::new(),
            delays: HashMap::new(),
            classified: Vec::new(),
            searches: Vec::new(),
            embedded: Vec::new(),
            presented: None,
        }))
    }

    /// Seeds `count` retrieved candidates titled `Candidate <rank>`, each with a current source.
    fn seed(world: &Shared, count: usize) -> Vec<ProductListingId> {
        let mut world = world.lock().unwrap();
        (0..count)
            .map(|rank| {
                let mut source = crate::product_match_evaluator::tests::product().unwrap();
                source
                    .titles
                    .insert(Language::En, format!("Candidate {rank}").into());
                world.result.items.push(search_item(&source));
                world.sources.insert(
                    ProductListingSearchFilterMatchSourceRef {
                        product_listing_id: source.product_listing_id,
                        event_id: source.event_id,
                    },
                    source.clone(),
                );
                source.product_listing_id
            })
            .collect()
    }

    fn search_item(source: &ProductListingSearchFilterMatchSource) -> ProductListingSearchItem {
        ProductListingSearchItem {
            geography: Default::default(),
            product_listing_id: source.product_listing_id,
            product_listing_title_slug_id: source.product_listing_title_slug_id.clone(),
            event_id: source.event_id,
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
        }
    }

    fn decide(world: &Shared, rank: usize, decision: Decision) {
        decide_in_turn(world, rank, vec![decision]);
    }

    fn decide_in_turn(world: &Shared, rank: usize, decisions: Vec<Decision>) {
        world
            .lock()
            .unwrap()
            .decisions
            .insert(format!("Candidate {rank}"), decisions);
    }

    fn delay(world: &Shared, rank: usize, delay: Duration) {
        world
            .lock()
            .unwrap()
            .delays
            .insert(format!("Candidate {rank}"), delay);
    }

    fn classifications_of(world: &Shared, rank: usize) -> usize {
        let title = format!("Candidate {rank}");
        classified_ranks(world)
            .iter()
            .filter(|classified| **classified == title)
            .count()
    }

    fn classified_ranks(world: &Shared) -> Vec<String> {
        world
            .lock()
            .unwrap()
            .classified
            .iter()
            .map(|request| {
                request.state["candidate"]["title"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }

    fn presented(world: &Shared) -> Option<Vec<ProductListingId>> {
        world.lock().unwrap().presented.clone()
    }

    fn text_search() -> ProductListingSearch {
        ProductListingSearch::new(Language::En, Currency::Eur)
            .with_product_listing_query("brass lamp".try_into().unwrap())
    }

    fn enhanced_search() -> ProductListingSearch {
        text_search().with_enhanced_search_description("only before 1930".try_into().unwrap())
    }

    fn owner(world: &Shared) -> UserId {
        world.lock().unwrap().owner
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("preview-test"),
            correlation_id: CorrelationId::new("preview-test"),
        }
    }

    type Handler = PreviewSearchFilterMatchesHandler<
        FakeUnitOfWork,
        FakeTiers,
        FakeEmbeddings,
        FakeFxRates,
        FakeSearch,
        FakeSources,
        FakeClassifier,
        FakePresenter,
    >;

    fn handler(world: &Shared, classification_deadline: Duration) -> Handler {
        PreviewSearchFilterMatchesHandler::new(
            FakeUnitOfWork,
            FakeTiers(Arc::clone(world)),
            FakeEmbeddings(Arc::clone(world)),
            FakeFxRates,
            FakeSearch(Arc::clone(world)),
            FakeSources(Arc::clone(world)),
            FakeClassifier(Arc::clone(world)),
            FakePresenter(Arc::clone(world)),
            SearchFilterMatchPreviewPolicy {
                should_show_threshold: Probability::new(0.5).unwrap(),
                classification_deadline,
            },
        )
    }

    async fn preview(
        world: &Shared,
        search: ProductListingSearch,
    ) -> Result<SearchFilterMatchPreview, PreviewSearchFilterMatchesError> {
        handler(world, Duration::from_secs(5))
            .execute(
                &context(Principal::User(owner(world))),
                PreviewSearchFilterMatchesCommand {
                    user_id: owner(world),
                    search,
                },
            )
            .await
    }

    #[tokio::test]
    async fn should_stop_classifying_once_four_candidates_match() {
        let world = world(UserTier::Pro);
        let ids = seed(&world, 20);
        for rank in 0..20 {
            decide(&world, rank, Decision::Accept);
        }

        let preview = preview(&world, text_search()).await.unwrap();

        assert!(!preview.candidate_limit_reached);
        assert!(classified_ranks(&world).len() < 4 + PREVIEW_CLASSIFICATION_CONCURRENCY);
        assert_eq!(Some(ids[..4].to_vec()), presented(&world));
    }

    #[tokio::test]
    async fn should_answer_once_four_matches_are_known_without_waiting_for_later_candidates() {
        let world = world(UserTier::Pro);
        let ids = seed(&world, 8);
        for rank in 0..8 {
            decide(&world, rank, Decision::Accept);
            if rank >= 4 {
                delay(&world, rank, Duration::from_secs(60));
            }
        }

        let preview = handler(&world, Duration::from_millis(500))
            .execute(
                &context(Principal::User(owner(&world))),
                PreviewSearchFilterMatchesCommand {
                    user_id: owner(&world),
                    search: text_search(),
                },
            )
            .await
            .unwrap();

        assert!(!preview.candidate_limit_reached);
        assert_eq!(Some(ids[..4].to_vec()), presented(&world));
    }

    #[tokio::test]
    async fn should_present_scattered_matches_in_rank_order() {
        let world = world(UserTier::Pro);
        let ids = seed(&world, 20);
        for rank in 0..20 {
            let accepted = [2, 5, 9, 12, 13].contains(&rank);
            decide(
                &world,
                rank,
                if accepted {
                    Decision::Accept
                } else {
                    Decision::Reject
                },
            );
        }

        let preview = preview(&world, text_search()).await.unwrap();

        assert!(!preview.candidate_limit_reached);
        let classified = classified_ranks(&world);
        assert!((0..=12).all(|rank| classified.contains(&format!("Candidate {rank}"))));
        assert!(classified.len() <= PREVIEW_EVALUATION_LIMIT);
        assert_eq!(
            Some(vec![ids[2], ids[5], ids[9], ids[12]]),
            presented(&world)
        );
    }

    #[tokio::test]
    async fn should_classify_at_most_sixteen_candidates_when_matches_are_rare() {
        let world = world(UserTier::Pro);
        seed(&world, 30);
        for rank in 0..30 {
            decide(
                &world,
                rank,
                if rank == 20 {
                    Decision::Accept
                } else {
                    Decision::Reject
                },
            );
        }

        let preview = preview(&world, text_search()).await.unwrap();

        assert!(preview.candidate_limit_reached);
        assert_eq!(
            (0..16)
                .map(|rank| format!("Candidate {rank}"))
                .collect::<Vec<_>>(),
            classified_ranks(&world)
        );
        assert_eq!(Some(Vec::new()), presented(&world));
    }

    #[tokio::test]
    async fn should_not_report_a_candidate_limit_when_retrieval_is_exhausted() {
        let world = world(UserTier::Pro);
        let ids = seed(&world, 3);
        decide(&world, 0, Decision::Reject);
        decide(&world, 1, Decision::Accept);
        decide(&world, 2, Decision::Reject);

        let preview = preview(&world, text_search()).await.unwrap();

        assert!(!preview.candidate_limit_reached);
        assert_eq!(Some(vec![ids[1]]), presented(&world));
    }

    #[tokio::test]
    async fn should_fail_as_unavailable_when_a_ranked_decision_stays_unknown_after_retries() {
        let world = world(UserTier::Pro);
        seed(&world, 6);
        decide(&world, 0, Decision::Accept);
        decide(
            &world,
            1,
            Decision::Fail(ClassificationError::RateLimited { retry_after: None }),
        );
        for rank in 2..6 {
            decide(&world, rank, Decision::Accept);
        }

        let error = preview(&world, text_search()).await.unwrap_err();

        assert!(matches!(
            error,
            PreviewSearchFilterMatchesError::ClassifierUnavailable {
                source: ClassificationError::RateLimited { .. }
            }
        ));
        assert_eq!(
            PREVIEW_RETRY_DELAYS.len() + 1,
            classifications_of(&world, 1)
        );
        assert_eq!(None, presented(&world));
    }

    #[tokio::test]
    async fn should_retry_an_undecided_candidate_before_deciding_it() {
        let world = world(UserTier::Pro);
        let ids = seed(&world, 1);
        decide_in_turn(
            &world,
            0,
            vec![
                Decision::Fail(ClassificationError::Transient),
                Decision::Accept,
            ],
        );

        preview(&world, text_search()).await.unwrap();

        assert_eq!(2, classifications_of(&world, 0));
        assert_eq!(Some(vec![ids[0]]), presented(&world));
    }

    #[tokio::test]
    async fn should_fail_without_retrying_when_the_classifier_rejects_its_configuration() {
        let world = world(UserTier::Pro);
        seed(&world, 1);
        decide(
            &world,
            0,
            Decision::Fail(ClassificationError::Authentication),
        );

        let error = preview(&world, text_search()).await.unwrap_err();

        assert!(matches!(
            error,
            PreviewSearchFilterMatchesError::ClassifierMisconfigured {
                source: ClassificationError::Authentication
            }
        ));
        assert_eq!(1, classifications_of(&world, 0));
    }

    #[tokio::test]
    async fn should_report_a_candidate_limit_when_retrieval_reaches_its_scan_limit() {
        let world = world(UserTier::Pro);
        let ids = seed(&world, PREVIEW_SCAN_LIMIT.get());
        {
            let mut world = world.lock().unwrap();
            for source in world.sources.values_mut() {
                if ids[..20].contains(&source.product_listing_id) {
                    source.current_event_id = domain_primitives::event_id::EventId::new();
                }
            }
        }
        for rank in 20..PREVIEW_SCAN_LIMIT.get() {
            decide(&world, rank, Decision::Reject);
        }

        let preview = preview(&world, text_search()).await.unwrap();

        assert!(preview.candidate_limit_reached);
        assert_eq!(12, classified_ranks(&world).len());
    }

    #[tokio::test]
    async fn should_skip_candidates_that_can_never_be_classified() {
        let world = world(UserTier::Pro);
        let ids = seed(&world, 2);
        decide(
            &world,
            0,
            Decision::Fail(ClassificationError::PermanentCandidateFailure),
        );
        decide(&world, 1, Decision::Accept);

        preview(&world, text_search()).await.unwrap();

        assert_eq!(Some(vec![ids[1]]), presented(&world));
    }

    #[tokio::test]
    async fn should_fail_as_unavailable_when_classification_exceeds_its_deadline() {
        let world = world(UserTier::Pro);
        seed(&world, 1);
        decide(&world, 0, Decision::Accept);
        delay(&world, 0, Duration::from_millis(200));

        let error = handler(&world, Duration::from_millis(10))
            .execute(
                &context(Principal::User(owner(&world))),
                PreviewSearchFilterMatchesCommand {
                    user_id: owner(&world),
                    search: text_search(),
                },
            )
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            PreviewSearchFilterMatchesError::ClassifierUnavailable {
                source: ClassificationError::Timeout
            }
        ));
    }

    #[tokio::test]
    async fn should_classify_with_the_evidence_policy_of_the_users_tier() {
        for (tier, expected_policy, expected_images) in [
            (UserTier::Free, "TEXT_ONLY", 0),
            (UserTier::Pro, "TEXT_AND_ONE_IMAGE", 1),
            (UserTier::Ultimate, "ENHANCED_TEXT_AND_FOUR_IMAGES", 4),
        ] {
            let world = world(tier);
            seed(&world, 1);
            for source in world.lock().unwrap().sources.values_mut() {
                source.images = (0..6)
                    .map(|index| {
                        product_listing_core::product_listing_image::ProductListingImage::new(
                            url::Url::parse(&format!("https://example.test/{index}.jpg")).unwrap(),
                        )
                    })
                    .collect();
            }
            decide(&world, 0, Decision::Accept);
            let search = if tier == UserTier::Ultimate {
                enhanced_search()
            } else {
                text_search()
            };

            preview(&world, search).await.unwrap();

            let world = world.lock().unwrap();
            let request = &world.classified[0];
            assert_eq!(
                expected_policy,
                request.state["evaluation_context"]["evidence_policy"]
            );
            assert_eq!(expected_images, request.image_urls.len());
            assert_eq!(
                tier == UserTier::Ultimate,
                request.state["search_intent"]["enhanced_description"].is_string()
            );
        }
    }

    #[tokio::test]
    async fn should_reject_criteria_the_users_tier_may_not_save_before_searching() {
        let world = world(UserTier::Free);
        seed(&world, 1);
        let mut search = text_search();
        search.shipping = Some(
            ShippingSearch::new(
                geo::reference::ReferenceRelease::CURRENT,
                None,
                None,
                None,
                true,
            )
            .unwrap(),
        );

        let error = preview(&world, search).await.unwrap_err();

        assert!(matches!(
            error,
            PreviewSearchFilterMatchesError::SearchFilterFeatureRestricted {
                feature: "shipping"
            }
        ));
        assert!(world.lock().unwrap().searches.is_empty());
    }

    #[tokio::test]
    async fn should_match_criteria_without_text_intent_without_the_classifier() {
        let world = world(UserTier::Free);
        let ids = seed(&world, 6);

        let preview = preview(
            &world,
            ProductListingSearch::new(Language::En, Currency::Eur),
        )
        .await
        .unwrap();

        assert!(!preview.candidate_limit_reached);
        assert!(classified_ranks(&world).is_empty());
        assert_eq!(Some(ids[..4].to_vec()), presented(&world));
    }

    #[tokio::test]
    async fn should_search_the_whole_catalogue_lexically_for_ordinary_intent() {
        let world = world(UserTier::Pro);
        seed(&world, 1);
        decide(&world, 0, Decision::Reject);

        preview(&world, text_search()).await.unwrap();

        let world = world.lock().unwrap();
        let [search] = world.searches.as_slice() else {
            panic!("expected exactly one retrieval");
        };
        assert!(!search.hybrid);
        assert_eq!(Some(32), search.size);
        assert_eq!(None, search.search.updated_query);
        assert!(world.embedded.is_empty());
    }

    #[tokio::test]
    async fn should_retrieve_enhanced_intent_with_the_embedding_of_the_saved_filter_text() {
        let world = world(UserTier::Ultimate);
        seed(&world, 1);
        decide(&world, 0, Decision::Reject);

        preview(&world, enhanced_search()).await.unwrap();

        let world = world.lock().unwrap();
        assert!(world.searches[0].hybrid);
        assert_eq!(
            vec![
                embedding_query(&enhanced_search())
                    .unwrap()
                    .unwrap()
                    .as_str()
                    .to_owned()
            ],
            world.embedded
        );
    }

    #[tokio::test]
    async fn should_skip_stale_withdrawn_and_missing_sources_without_spending_the_budget() {
        let world = world(UserTier::Pro);
        let ids = seed(&world, 4);
        {
            let mut world = world.lock().unwrap();
            let refs = world.sources.keys().copied().collect::<Vec<_>>();
            for reference in refs {
                let source = world.sources.get_mut(&reference).unwrap();
                if reference.product_listing_id == ids[0] {
                    source.current_event_id = domain_primitives::event_id::EventId::new();
                } else if reference.product_listing_id == ids[1] {
                    source.lifecycle = ListingLifecycle::Withdrawn;
                }
            }
            world
                .sources
                .retain(|reference, _| reference.product_listing_id != ids[2]);
        }
        decide(&world, 3, Decision::Accept);

        preview(&world, text_search()).await.unwrap();

        assert_eq!(vec!["Candidate 3".to_owned()], classified_ranks(&world));
        assert_eq!(Some(vec![ids[3]]), presented(&world));
    }

    #[tokio::test]
    async fn should_only_preview_for_the_owner_with_search_filter_write_access() {
        let world = world(UserTier::Pro);
        let read_only = Principal::DelegatedUser {
            user_id: owner(&world),
            capabilities: BTreeSet::from([CredentialCapability::SearchFiltersRead]),
        };
        for (principal, expected_authentication_failure) in [
            (Principal::Anonymous, true),
            (Principal::User(UserId::new()), false),
            (read_only, false),
        ] {
            let error = handler(&world, Duration::from_secs(5))
                .execute(
                    &context(principal),
                    PreviewSearchFilterMatchesCommand {
                        user_id: owner(&world),
                        search: text_search(),
                    },
                )
                .await
                .unwrap_err();

            assert_eq!(
                expected_authentication_failure,
                matches!(
                    error,
                    PreviewSearchFilterMatchesError::AuthenticatedActorRequired
                )
            );
            assert_eq!(
                !expected_authentication_failure,
                matches!(
                    error,
                    PreviewSearchFilterMatchesError::ActorMayNotManageSearchFilter
                )
            );
        }
        assert!(world.lock().unwrap().searches.is_empty());
    }

    struct FakeUnitOfWork;
    struct FakeTransaction;

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            Ok(FakeTransaction)
        }
    }

    struct FakeTiers(Shared);

    #[async_trait::async_trait]
    impl UserTierEntitlements for FakeTiers {
        async fn lock_user_tier(
            &mut self,
            _: UserId,
        ) -> Result<Option<UserTier>, UserTierEntitlementsError> {
            Ok(Some(self.0.lock().unwrap().tier))
        }

        async fn reconcile_for_tier(
            &mut self,
            _: UserId,
            _: UserTier,
        ) -> Result<(), UserTierEntitlementsError> {
            panic!("previews never reconcile entitlements")
        }
    }

    impl UserTierEntitlementsFactory<FakeTransaction> for FakeTiers {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut FakeTransaction,
        ) -> impl UserTierEntitlements + 'tx {
            FakeTiers(Arc::clone(&self.0))
        }
    }

    struct FakeEmbeddings(Shared);

    #[async_trait::async_trait]
    impl EmbeddingGenerator for FakeEmbeddings {
        async fn embed_product(
            &self,
            _: &EmbeddingText,
            _: Option<&EmbeddingText>,
            _: Option<&EmbeddingImageUrl>,
        ) -> Result<EmbeddingVector, EmbeddingError> {
            panic!("previews never embed products")
        }

        async fn embed_search_query(
            &self,
            query: &EmbeddingText,
        ) -> Result<EmbeddingVector, EmbeddingError> {
            self.0
                .lock()
                .unwrap()
                .embedded
                .push(query.as_str().to_owned());
            EmbeddingVector::try_new(vec![0.5; embedding::EMBEDDING_DIMENSIONS])
        }
    }

    struct FakeFxRates;

    #[async_trait::async_trait]
    impl FxRateSnapshotReader for FakeFxRates {
        async fn find_by_id(
            &self,
            _: FxRateId,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError> {
            panic!("previews read the latest snapshot")
        }

        async fn find_latest_at_or_before(
            &self,
            _: OffsetDateTime,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError> {
            Ok(Some(snapshot()))
        }

        async fn find_latest_at_or_before_many(
            &self,
            _: &[OffsetDateTime],
        ) -> Result<Vec<FxRateSnapshot>, FxRateSnapshotReadError> {
            panic!("previews read one snapshot")
        }
    }

    fn snapshot() -> FxRateSnapshot {
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
        .unwrap()
        .into_persisted(FxRateGeneration::try_from(1).unwrap())
    }

    struct FakeSearch(Shared);

    impl FakeSearch {
        fn record(
            &self,
            request: &ProductListingSearchReadRequest,
            hybrid: bool,
        ) -> ProductListingSearchReadResult {
            let mut world = self.0.lock().unwrap();
            world.searches.push(RecordedSearch {
                search: request.compiled_search.search.clone(),
                size: request.cursor.as_ref().map(|cursor| cursor.size),
                hybrid,
            });
            world.result.clone()
        }
    }

    #[async_trait::async_trait]
    impl ProductListingSearchReader for FakeSearch {
        fn continuation_mode(
            &self,
            _: &serde_json::Value,
            _: product_listing_core::sort_product_listing_field::SortProductListingField,
        ) -> Result<ProductListingSearchMode, ProductListingSearchReadError> {
            panic!("previews never continue a search")
        }

        async fn search(
            &self,
            request: &ProductListingSearchReadRequest,
        ) -> Result<ProductListingSearchReadResult, ProductListingSearchReadError> {
            Ok(self.record(request, false))
        }

        async fn search_hybrid(
            &self,
            request: &ProductListingSearchReadRequest,
            _: &[f32],
        ) -> Result<ProductListingSearchReadResult, ProductListingSearchReadError> {
            Ok(self.record(request, true))
        }
    }

    struct FakeSources(Shared);

    #[async_trait::async_trait]
    impl ProductListingSearchFilterMatchSourceReader for FakeSources {
        async fn find_source(
            &mut self,
            event_id: domain_primitives::event_id::EventId,
            product_listing_id: ProductListingId,
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
                    product_listing_id,
                    event_id,
                })
                .cloned())
        }
    }

    impl ProductListingSearchFilterMatchSourceReaderFactory<FakeTransaction> for FakeSources {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut FakeTransaction,
        ) -> impl ProductListingSearchFilterMatchSourceReader + 'tx {
            FakeSources(Arc::clone(&self.0))
        }
    }

    struct FakeClassifier(Shared);

    #[async_trait::async_trait]
    impl ClassifierModel for FakeClassifier {
        async fn classify(
            &self,
            request: ClassificationRequest,
        ) -> Result<ClassificationResponse, ClassificationError> {
            let title = request.state["candidate"]["title"]
                .as_str()
                .unwrap()
                .to_owned();
            let (delay, decision) = {
                let mut world = self.0.lock().unwrap();
                world.classified.push(request);
                let delay = world.delays.get(&title).copied().unwrap_or_default();
                let decisions = world.decisions.get_mut(&title).unwrap();
                let decision = if decisions.len() > 1 {
                    decisions.remove(0)
                } else {
                    decisions[0].clone()
                };
                (delay, decision)
            };
            tokio::time::sleep(delay).await;
            let score = match decision {
                Decision::Accept => 0.75,
                Decision::Reject => 0.1,
                Decision::Fail(error) => return Err(error),
            };
            Ok(ClassificationResponse {
                answers: std::collections::BTreeMap::from([(
                    QuestionId::new("should_show").unwrap(),
                    Probability::new(score).unwrap(),
                )]),
                usage: Default::default(),
                diagnostics: Default::default(),
            })
        }
    }

    struct FakePresenter(Shared);

    #[async_trait::async_trait]
    impl ProductListingSearchPresenter for FakePresenter {
        async fn present(
            &self,
            _: &OperationContext,
            items: Vec<ProductListingSearchItem>,
            _: &ProductListingSearch,
            _: &FxRateSnapshot,
        ) -> Result<Vec<PersonalizedProductListingSummary>, SearchProductListingsError> {
            let ids = items.iter().map(|item| item.product_listing_id).collect();
            let unique = items
                .iter()
                .map(|item| item.product_listing_id)
                .collect::<HashSet<_>>();
            assert_eq!(
                unique.len(),
                items.len(),
                "previews present each listing once"
            );
            self.0.lock().unwrap().presented = Some(ids);
            Ok(Vec::new())
        }
    }
}
