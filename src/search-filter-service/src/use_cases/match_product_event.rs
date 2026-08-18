use crate::ports::{
    ActiveSearchFilterMatchCandidate, ActiveSearchFilterMatchCandidateReadError,
    ActiveSearchFilterMatchCandidateReader, ActiveSearchFilterMatchCandidateReaderFactory,
    SearchFilterIndex, SearchFilterIndexError, SearchFilterMatchCandidate,
    SearchFilterMatchPersistOutcome, SearchFilterMatchWriteError, SearchFilterMatchWriter,
    SearchFilterMatchWriterFactory,
};
use common::error::boxed::{BoxError, box_error};
use common::event_id::EventId;
use common::fx_rate_id::FxRateId;
use common::product_id::ProductId;
use common::resource_state::domain::ResourceState;
use common::transaction::{Transaction, UnitOfWork};
#[cfg(test)]
use fxrate_core::FxRateSnapshot;
use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use large_language_model::{
    BatchGenerationOptions, GenerationOptions, LargeLanguageModel, LargeLanguageModelError,
    StructuredGenerationRequest,
};
use product_core::product::ProductPriceValuationBasis;
use product_service::ports::{
    ProductCurrentRevisionCheck, ProductCurrentRevisionCheckError, ProductCurrentRevisionGuard,
    ProductCurrentRevisionGuardFactory, ProductPercolationInput, ProductPercolationValuation,
    ProductPricesByCurrency, ProductSearchFilterMatchSource,
    ProductSearchFilterMatchSourceReadError, ProductSearchFilterMatchSourceReader,
    ProductSearchFilterMatchSourceReaderFactory,
};
use search_filter_core::{PriceMatchValuation, SearchFilterProductMatch};
use serde::Deserialize;
use std::num::NonZeroUsize;

const MAX_CONCURRENT_LLM_REQUESTS: NonZeroUsize = match NonZeroUsize::new(4) {
    Some(value) => value,
    None => NonZeroUsize::MIN,
};
const MAX_PRODUCT_MATCH_IMAGES: usize = 5;
const PRODUCT_MATCH_SYSTEM_INSTRUCTION: &str = "You are a product matching assistant for an antiques marketplace. Decide whether the product actually matches the requested search description using the product title, description, and optional product images. Return only JSON with a boolean `matches` and, when `matches` is true, a compact user-facing `reason` in the search language. Do not include markdown or extra fields.";

#[derive(Debug, Clone, PartialEq)]
pub struct MatchProductEventCommand {
    pub origin_event_id: EventId,
    pub product_id: ProductId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchProductEventOutcome {
    Processed,
    DuplicateAlreadyPersisted,
    StaleSourceSkipped,
    SourceNotFound,
    IgnoredEventType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchProductEventResult {
    pub outcome: MatchProductEventOutcome,
    pub percolated_count: usize,
    pub persisted_match_count: usize,
    /// Enhanced candidates that were not evaluated. Their failures are explicit
    /// operational outcomes; they never make a plain percolation match implicit.
    pub enhanced_evaluation_failure_count: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum MatchProductEventError {
    #[error("failed to begin product source read transaction")]
    BeginSourceReadTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("product source read failed")]
    ProductSourceReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("product source persisted state is invalid")]
    ProductSourceStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("product source does not match requested event or product")]
    ProductSourceMismatch,
    #[error("sale FX snapshot is missing from canonical persisted storage")]
    SaleSnapshotNotFound { fx_rate_id: FxRateId },
    #[error("event-effective FX snapshot is missing from canonical persisted storage")]
    EventSnapshotNotFound {
        origin_event_time: time::OffsetDateTime,
    },
    #[error("sale FX snapshot read failed")]
    SaleSnapshotReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("event-effective FX snapshot read failed")]
    EventSnapshotReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("sale FX snapshot persisted state is invalid")]
    SaleSnapshotStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("event-effective FX snapshot persisted state is invalid")]
    EventSnapshotStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("event-time FX conversion failed")]
    EventValuationConversionFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to commit product source read transaction")]
    CommitSourceReadTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter percolation failed")]
    PercolationFailed {
        #[source]
        source: BoxError,
    },
    #[error("product match evaluation failed")]
    ProductMatchEvaluationFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin search filter match transaction")]
    BeginTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("product current revision check failed")]
    ProductRevisionCheckFailed {
        #[source]
        source: BoxError,
    },
    #[error("active search filter match candidate read failed")]
    CandidateReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("active search filter match candidate state is invalid")]
    CandidateStateInvalid {
        #[source]
        source: BoxError,
    },

    #[error("search filter match persistence failed")]
    MatchPersistenceFailed {
        #[source]
        source: BoxError,
    },
    #[error("persisted search filter match state is invalid")]
    PersistedMatchStateInvalid {
        #[source]
        source: BoxError,
    },

    #[error("failed to commit search filter match transaction")]
    CommitTransactionFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait MatchProductEventUseCase: Send + Sync {
    async fn execute(
        &self,
        command: MatchProductEventCommand,
    ) -> Result<MatchProductEventResult, MatchProductEventError>;
}

pub struct MatchProductEventHandler<U, S, G, F, I, E, R, W> {
    unit_of_work: U,
    sources: S,
    revisions: G,
    fx_rates: F,
    index: I,
    evaluator: E,
    candidates: R,
    matches: W,
}

impl<U, S, G, F, I, E, R, W> MatchProductEventHandler<U, S, G, F, I, E, R, W> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        unit_of_work: U,
        sources: S,
        revisions: G,
        fx_rates: F,
        index: I,
        evaluator: E,
        candidates: R,
        matches: W,
    ) -> Self {
        Self {
            unit_of_work,
            sources,
            revisions,
            fx_rates,
            index,
            evaluator,
            candidates,
            matches,
        }
    }
}

#[async_trait::async_trait]
impl<U, S, G, F, I, E, R, W> MatchProductEventUseCase
    for MatchProductEventHandler<U, S, G, F, I, E, R, W>
where
    U: UnitOfWork,
    S: ProductSearchFilterMatchSourceReaderFactory<U::Tx>,
    G: ProductCurrentRevisionGuardFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
    I: SearchFilterIndex,
    E: LargeLanguageModel,
    R: ActiveSearchFilterMatchCandidateReaderFactory<U::Tx>,
    W: SearchFilterMatchWriterFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "match_product_event",
        skip_all,
        fields(
            origin_event_id = %command.origin_event_id,
            product_id = %command.product_id,
        )
    )]
    async fn execute(
        &self,
        command: MatchProductEventCommand,
    ) -> Result<MatchProductEventResult, MatchProductEventError> {
        let product =
            load_product_source(&self.unit_of_work, &self.sources, &self.fx_rates, &command)
                .await?;
        let product = match product {
            ProductSourceReadOutcome::Missing => {
                return Ok(MatchProductEventResult {
                    outcome: MatchProductEventOutcome::SourceNotFound,
                    percolated_count: 0,
                    persisted_match_count: 0,
                    enhanced_evaluation_failure_count: 0,
                });
            }
            ProductSourceReadOutcome::IgnoredEventType => {
                return Ok(MatchProductEventResult {
                    outcome: MatchProductEventOutcome::IgnoredEventType,
                    percolated_count: 0,
                    persisted_match_count: 0,
                    enhanced_evaluation_failure_count: 0,
                });
            }
            ProductSourceReadOutcome::Stale => {
                return Ok(MatchProductEventResult {
                    outcome: MatchProductEventOutcome::StaleSourceSkipped,
                    percolated_count: 0,
                    persisted_match_count: 0,
                    enhanced_evaluation_failure_count: 0,
                });
            }
            ProductSourceReadOutcome::Current(product) => *product,
        };

        let price_match_valuation =
            product
                .valuation
                .as_ref()
                .map(|valuation| PriceMatchValuation {
                    basis: valuation.basis,
                    fx_rate_id: valuation.fx_rate_id,
                });
        let percolated = self
            .index
            .percolate(&product)
            .await
            .map_err(percolation_error)?;
        let percolated_count = percolated.len();
        let evaluated = evaluate_candidates(
            &self.evaluator,
            &product.source,
            percolated,
            price_match_valuation,
        )
        .await;
        let candidates = evaluated.candidates;
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            MatchProductEventError::BeginTransactionFailed {
                source: box_error(source),
            }
        })?;
        let revision = self
            .revisions
            .in_transaction(&mut tx)
            .lock_and_check(command.product_id, command.origin_event_id)
            .await
            .map_err(product_revision_check_error)?;
        if revision == ProductCurrentRevisionCheck::Stale {
            return Ok(MatchProductEventResult {
                outcome: MatchProductEventOutcome::StaleSourceSkipped,
                percolated_count,
                persisted_match_count: 0,
                enhanced_evaluation_failure_count: evaluated.enhanced_evaluation_failure_count,
            });
        }

        let mut candidates = if candidates.is_empty() {
            Vec::new()
        } else {
            self.candidates
                .in_transaction(&mut tx)
                .find_active(&candidates)
                .await
                .map_err(candidate_read_error)?
        };
        sort_candidates(&mut candidates);

        let mut persisted_match_count = 0;
        let mut duplicate_match_count = 0;
        for candidate in candidates {
            let product_match = SearchFilterProductMatch {
                user_id: candidate.user_id,
                user_search_filter_id: candidate.search_filter_id,
                user_search_filter_name: Some(candidate.search_filter_name),
                product_id: command.product_id,
                origin_event_id: command.origin_event_id,
                price_match_valuation: candidate.price_match_valuation,
                enhanced_match_reason: candidate.enhanced_match_reason,
                feedback: None,
            };
            let outcome = self
                .matches
                .in_transaction(&mut tx)
                .insert_if_absent(&product_match)
                .await
                .map_err(match_write_error)?;
            match outcome {
                SearchFilterMatchPersistOutcome::Inserted => persisted_match_count += 1,
                SearchFilterMatchPersistOutcome::AlreadyExists => duplicate_match_count += 1,
            }
        }

        tx.commit()
            .await
            .map_err(|source| MatchProductEventError::CommitTransactionFailed {
                source: box_error(source),
            })?;

        if let Some(error) = evaluated.retryable_error {
            return Err(product_match_evaluation_error(error));
        }

        Ok(MatchProductEventResult {
            outcome: if persisted_match_count == 0 && duplicate_match_count > 0 {
                MatchProductEventOutcome::DuplicateAlreadyPersisted
            } else {
                MatchProductEventOutcome::Processed
            },
            percolated_count,
            persisted_match_count,
            enhanced_evaluation_failure_count: evaluated.enhanced_evaluation_failure_count,
        })
    }
}

enum ProductSourceReadOutcome {
    Missing,
    IgnoredEventType,
    Stale,
    Current(Box<ProductPercolationSource>),
}

type ProductPercolationSource = ProductPercolationInput;

async fn load_product_source<U, S, F>(
    unit_of_work: &U,
    sources: &S,
    fx_rates: &F,
    command: &MatchProductEventCommand,
) -> Result<ProductSourceReadOutcome, MatchProductEventError>
where
    U: UnitOfWork,
    S: ProductSearchFilterMatchSourceReaderFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
{
    let mut tx = unit_of_work.begin().await.map_err(|source| {
        MatchProductEventError::BeginSourceReadTransactionFailed {
            source: box_error(source),
        }
    })?;
    let source = sources
        .in_transaction(&mut tx)
        .find_source(command.origin_event_id, command.product_id)
        .await
        .map_err(product_source_read_error)?;
    let outcome = match source {
        None => ProductSourceReadOutcome::Missing,
        Some(product)
            if product.event_id != command.origin_event_id
                || product.product_id != command.product_id =>
        {
            return Err(MatchProductEventError::ProductSourceMismatch);
        }
        Some(product) if !product.event_kind.is_percolation_trigger() => {
            ProductSourceReadOutcome::IgnoredEventType
        }
        Some(product) if product.current_event_id != command.origin_event_id => {
            ProductSourceReadOutcome::Stale
        }
        Some(product) => {
            let valuation = match product.pricing.price {
                None => None,
                Some(source_price) => {
                    let (basis, snapshot) = match product.sale_valuation {
                        Some(sale) => (
                            ProductPriceValuationBasis::Sale,
                            fx_rates
                                .in_transaction(&mut tx)
                                .find_by_id(sale.fx_rate_id)
                                .await
                                .map_err(sale_snapshot_read_error)?
                                .ok_or(MatchProductEventError::SaleSnapshotNotFound {
                                    fx_rate_id: sale.fx_rate_id,
                                })?,
                        ),
                        None => (
                            ProductPriceValuationBasis::Event,
                            fx_rates
                                .in_transaction(&mut tx)
                                .find_latest_at_or_before(product.origin_event_time)
                                .await
                                .map_err(event_snapshot_read_error)?
                                .ok_or(MatchProductEventError::EventSnapshotNotFound {
                                    origin_event_time: product.origin_event_time,
                                })?,
                        ),
                    };
                    let prices = ProductPricesByCurrency::convert_all(&snapshot, source_price)
                        .map_err(|source| {
                            MatchProductEventError::EventValuationConversionFailed {
                                source: box_error(source),
                            }
                        })?;
                    Some(ProductPercolationValuation {
                        basis,
                        fx_rate_id: snapshot.id(),
                        effective_at: snapshot.captured_at(),
                        prices,
                    })
                }
            };
            ProductSourceReadOutcome::Current(Box::new(ProductPercolationInput {
                source: product,
                valuation,
            }))
        }
    };
    tx.commit().await.map_err(|source| {
        MatchProductEventError::CommitSourceReadTransactionFailed {
            source: box_error(source),
        }
    })?;
    Ok(outcome)
}

struct EvaluatedCandidates {
    candidates: Vec<SearchFilterMatchCandidate>,
    enhanced_evaluation_failure_count: usize,
    retryable_error: Option<LargeLanguageModelError>,
}

#[derive(Debug, Deserialize)]
struct ProductMatchDecision {
    matches: bool,
    #[serde(default)]
    reason: Option<String>,
}

async fn evaluate_candidates<E>(
    llm: &E,
    product: &ProductSearchFilterMatchSource,
    mut filters: Vec<crate::ports::SearchFilterView>,
    price_match_valuation: Option<PriceMatchValuation>,
) -> EvaluatedCandidates
where
    E: LargeLanguageModel,
{
    filters.retain(|filter| filter.state == ResourceState::Active);
    filters.sort_by_key(|filter| filter.search_filter_id.to_string());
    filters.dedup_by(|left, right| left.search_filter_id == right.search_filter_id);

    let enhanced_filters = filters
        .iter()
        .filter(|filter| filter.search.enhanced_search_description.is_some())
        .cloned()
        .collect::<Vec<_>>();
    let mut requests = Vec::with_capacity(enhanced_filters.len());
    let mut enhanced_evaluations = std::collections::HashMap::new();
    for filter in enhanced_filters {
        match enhanced_filter_request(product, &filter) {
            Ok(request) => requests.push((filter.search_filter_id, request)),
            Err(error) => {
                enhanced_evaluations.insert(filter.search_filter_id, Err(error));
            }
        }
    }
    let results = llm
        .generate_batch::<ProductMatchDecision>(
            requests
                .iter()
                .map(|(_, request)| request.clone())
                .collect(),
            BatchGenerationOptions::new(MAX_CONCURRENT_LLM_REQUESTS),
        )
        .await;
    for ((search_filter_id, _), result) in requests.into_iter().zip(results) {
        enhanced_evaluations.insert(search_filter_id, result.and_then(product_match_reason));
    }

    let mut candidates = Vec::with_capacity(filters.len());
    let mut enhanced_evaluation_failure_count = 0;
    let mut retryable_error = None;
    for filter in filters {
        let enhanced_match_reason = if filter.search.enhanced_search_description.is_some() {
            match enhanced_evaluations.remove(&filter.search_filter_id) {
                Some(Ok(Some(reason))) => Some(reason),
                Some(Ok(None)) => continue,
                Some(Err(error)) => {
                    enhanced_evaluation_failure_count += 1;
                    let retryable = is_retryable_llm_error(&error);
                    tracing::warn!(
                        user_search_filter_id = %filter.search_filter_id,
                        error_category = %error,
                        "enhanced product match evaluation failed; plain and successful candidates remain eligible"
                    );
                    if retryable && retryable_error.is_none() {
                        retryable_error = Some(error);
                    }
                    continue;
                }
                None => {
                    enhanced_evaluation_failure_count += 1;
                    tracing::warn!(
                        user_search_filter_id = %filter.search_filter_id,
                        "enhanced product match evaluator omitted a candidate; plain and successful candidates remain eligible"
                    );
                    continue;
                }
            }
        } else {
            None
        };
        candidates.push(SearchFilterMatchCandidate {
            user_id: filter.user_id,
            search_filter_id: filter.search_filter_id,
            price_match_valuation: if filter.search.price_query.is_some() {
                price_match_valuation
            } else {
                None
            },
            enhanced_match_reason,
        });
    }
    EvaluatedCandidates {
        candidates,
        enhanced_evaluation_failure_count,
        retryable_error,
    }
}

fn sort_candidates(candidates: &mut Vec<ActiveSearchFilterMatchCandidate>) {
    candidates.sort_by_key(|candidate| {
        (
            candidate.user_id.to_string(),
            candidate.search_filter_id.to_string(),
        )
    });
    candidates.dedup_by(|left, right| {
        left.user_id == right.user_id && left.search_filter_id == right.search_filter_id
    });
}

fn product_source_read_error(
    error: ProductSearchFilterMatchSourceReadError,
) -> MatchProductEventError {
    match error {
        ProductSearchFilterMatchSourceReadError::InvalidPersistedState { source } => {
            MatchProductEventError::ProductSourceStateInvalid { source }
        }
        error => MatchProductEventError::ProductSourceReadFailed {
            source: box_error(error),
        },
    }
}

fn sale_snapshot_read_error(error: FxRateSnapshotRepositoryError) -> MatchProductEventError {
    match error {
        FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { source } => {
            MatchProductEventError::SaleSnapshotStateInvalid { source }
        }
        error => MatchProductEventError::SaleSnapshotReadFailed {
            source: box_error(error),
        },
    }
}

fn event_snapshot_read_error(error: FxRateSnapshotRepositoryError) -> MatchProductEventError {
    match error {
        FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { source } => {
            MatchProductEventError::EventSnapshotStateInvalid { source }
        }
        error => MatchProductEventError::EventSnapshotReadFailed {
            source: box_error(error),
        },
    }
}

fn percolation_error(error: SearchFilterIndexError) -> MatchProductEventError {
    MatchProductEventError::PercolationFailed {
        source: box_error(error),
    }
}

fn product_match_evaluation_error(error: LargeLanguageModelError) -> MatchProductEventError {
    MatchProductEventError::ProductMatchEvaluationFailed {
        source: box_error(error),
    }
}

fn enhanced_filter_request(
    product: &ProductSearchFilterMatchSource,
    filter: &crate::ports::SearchFilterView,
) -> Result<StructuredGenerationRequest, LargeLanguageModelError> {
    let description = filter
        .search
        .enhanced_search_description
        .as_ref()
        .ok_or_else(|| LargeLanguageModelError::InvalidResponse {
            source: box_error(std::io::Error::other("filter has no enhanced description")),
        })?;
    let language = filter.search.language;
    let (title, product_description) = product_text(product, language);
    let prompt = format!(
        "User's search description: {description}\nProduct title: {title}\nProduct description: {product_description}\nSearch language: {}\nReturn the reason in the search language.",
        language.format_human_readable(),
    );
    Ok(StructuredGenerationRequest {
        operation: common::logging::LlmOperation::ProductEnhancedSearchDescriptionMatching,
        system_instruction: PRODUCT_MATCH_SYSTEM_INSTRUCTION.to_owned(),
        prompt,
        image_urls: product
            .images
            .iter()
            .take(MAX_PRODUCT_MATCH_IMAGES)
            .map(|image| image.url.clone())
            .collect(),
        response_schema: product_match_response_schema(),
        options: GenerationOptions {
            temperature: 0.0,
            max_output_tokens: 256,
        },
    })
}

fn product_match_reason(
    decision: ProductMatchDecision,
) -> Result<Option<common::enhanced_match_reason::EnhancedMatchReason>, LargeLanguageModelError> {
    if !decision.matches {
        return Ok(None);
    }
    decision
        .reason
        .filter(|reason| !reason.trim().is_empty())
        .map(common::enhanced_match_reason::EnhancedMatchReason::from)
        .map(Some)
        .ok_or_else(|| LargeLanguageModelError::InvalidResponse {
            source: box_error(std::io::Error::other("matched response has no reason")),
        })
}

fn product_match_response_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "OBJECT",
        "properties": {
            "matches": {"type": "BOOLEAN"},
            "reason": {"type": "STRING"}
        },
        "required": ["matches"]
    })
}

fn product_text(
    product: &ProductSearchFilterMatchSource,
    search_language: common::language::domain::Language,
) -> (&str, &str) {
    let title = product
        .titles
        .get(&search_language)
        .or_else(|| product.titles.get(&common::language::domain::Language::En))
        .map(AsRef::as_ref)
        .or_else(|| {
            product
                .product_title
                .as_ref()
                .map(|title| title.payload.as_ref())
        })
        .unwrap_or("");
    let description = product
        .descriptions
        .get(&search_language)
        .or_else(|| {
            product
                .descriptions
                .get(&common::language::domain::Language::En)
        })
        .map(AsRef::as_ref)
        .unwrap_or("");
    (title, description)
}

fn is_retryable_llm_error(error: &LargeLanguageModelError) -> bool {
    matches!(
        error,
        LargeLanguageModelError::Timeout { .. }
            | LargeLanguageModelError::Retryable { .. }
            | LargeLanguageModelError::InvalidResponse { .. }
    )
}

fn product_revision_check_error(error: ProductCurrentRevisionCheckError) -> MatchProductEventError {
    MatchProductEventError::ProductRevisionCheckFailed {
        source: box_error(error),
    }
}

fn candidate_read_error(
    error: ActiveSearchFilterMatchCandidateReadError,
) -> MatchProductEventError {
    match error {
        ActiveSearchFilterMatchCandidateReadError::InvalidPersistedState { source } => {
            MatchProductEventError::CandidateStateInvalid { source }
        }
        error => MatchProductEventError::CandidateReadFailed {
            source: box_error(error),
        },
    }
}

fn match_write_error(error: SearchFilterMatchWriteError) -> MatchProductEventError {
    match error {
        SearchFilterMatchWriteError::InvalidPersistedState { source } => {
            MatchProductEventError::PersistedMatchStateInvalid { source }
        }
        error => MatchProductEventError::MatchPersistenceFailed {
            source: box_error(error),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        SearchFilterIndexQuery, SearchFilterProjectionWriteOutcome, SearchFilterView,
    };
    use common::{
        currency::domain::Currency,
        language::domain::Language,
        price::domain::{MonetaryAmount, Price},
        product_lifecycle::domain::ProductLifecycle,
        product_slug_id::ProductSlugId,
        product_state::domain::ProductState,
        query::range_query::RangeQuery,
        shop_id::ShopId,
        shop_name::ShopName,
        shop_slug_id::ShopSlugId,
        shops_product_id::ShopsProductId,
        transaction::TransactionError,
        user_id::UserId,
        user_search_filter_id::UserSearchFilterId,
        user_search_filter_name::UserSearchFilterName,
    };
    use fxrate_core::{
        FX_RATE_SCALE, FxRateGeneration, FxRateQuote, FxRateSource, NewFxRateSnapshot,
    };
    use fxrate_service::ports::{
        FxRateSnapshotInsertOutcome, FxRateSnapshotRepository, FxRateSnapshotRepositoryError,
        FxRateSnapshotRepositoryFactory,
    };
    use indexmap::IndexSet;
    use product_core::{
        product::{ProductAddress, ProductAuction, ProductPricing, ProductSaleValuation},
        product_image::ProductImage,
    };
    use product_service::ports::{
        ProductCurrentRevisionCheck, ProductCurrentRevisionCheckError, ProductCurrentRevisionGuard,
        ProductCurrentRevisionGuardFactory, ProductSearchFilterMatchShopType,
        ProductSearchFilterMatchSource, ProductSearchFilterMatchSourceEventKind,
    };
    use std::sync::{Arc, Mutex};
    use strum::IntoEnumIterator;
    use tokio::sync::Notify;

    use time::OffsetDateTime;
    use url::Url;

    #[derive(Default)]
    struct State {
        committed: usize,
        persisted: Vec<SearchFilterProductMatch>,
        active_reads: usize,
        sale_snapshot_reads: usize,
        event_snapshot_reads: usize,
        sale_snapshot: Option<FxRateSnapshot>,
        event_snapshot: Option<FxRateSnapshot>,
        current_event_id: Option<EventId>,
    }

    #[derive(Clone, Default)]
    struct FakeUnitOfWork(Arc<Mutex<State>>);

    struct FakeTransaction(Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = self.0.lock().map_err(|_| TransactionError::CommitFailed)?;
            state.committed += 1;
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

    struct Sources(Vec<ProductSearchFilterMatchSource>);

    struct ReadingSource(Vec<ProductSearchFilterMatchSource>);

    #[async_trait::async_trait]
    impl ProductSearchFilterMatchSourceReader for ReadingSource {
        async fn find_source(
            &mut self,
            event_id: EventId,
            product_id: ProductId,
        ) -> Result<Option<ProductSearchFilterMatchSource>, ProductSearchFilterMatchSourceReadError>
        {
            Ok(self
                .0
                .iter()
                .find(|source| source.event_id == event_id && source.product_id == product_id)
                .cloned())
        }
    }

    impl ProductSearchFilterMatchSourceReaderFactory<FakeTransaction> for Sources {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ProductSearchFilterMatchSourceReader + 'tx {
            ReadingSource(self.0.clone())
        }
    }

    #[derive(Clone)]
    struct Revisions(Arc<Mutex<State>>);

    struct CheckingRevision<'a>(&'a Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl ProductCurrentRevisionGuard for CheckingRevision<'_> {
        async fn lock_and_check(
            &mut self,
            _product_id: ProductId,
            expected_event_id: EventId,
        ) -> Result<ProductCurrentRevisionCheck, ProductCurrentRevisionCheckError> {
            let state =
                self.0
                    .lock()
                    .map_err(|_| ProductCurrentRevisionCheckError::CheckFailed {
                        source: box_error(std::io::Error::other("test mutex poisoned")),
                    })?;
            Ok(match state.current_event_id {
                Some(current_event_id) if current_event_id != expected_event_id => {
                    ProductCurrentRevisionCheck::Stale
                }
                Some(_) | None => ProductCurrentRevisionCheck::Current,
            })
        }
    }

    impl ProductCurrentRevisionGuardFactory<FakeTransaction> for Revisions {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ProductCurrentRevisionGuard + 'tx {
            CheckingRevision(&self.0)
        }
    }

    struct Index {
        filters: Vec<SearchFilterView>,
    }

    #[async_trait::async_trait]
    impl SearchFilterIndex for Index {
        async fn upsert(
            &self,
            _projection: &crate::ports::SearchFilterProjection,
        ) -> Result<SearchFilterProjectionWriteOutcome, SearchFilterIndexError> {
            Ok(SearchFilterProjectionWriteOutcome::Applied)
        }

        async fn delete(
            &self,
            _id: UserSearchFilterId,
            _source_version: i64,
        ) -> Result<SearchFilterProjectionWriteOutcome, SearchFilterIndexError> {
            Ok(SearchFilterProjectionWriteOutcome::Applied)
        }

        async fn percolate(
            &self,
            _input: &ProductPercolationInput,
        ) -> Result<Vec<SearchFilterView>, SearchFilterIndexError> {
            Ok(self.filters.clone())
        }

        async fn query(
            &self,
            _query: &SearchFilterIndexQuery,
        ) -> Result<
            common::pagination::cursor::CursoredResult<SearchFilterView, serde_json::Value>,
            SearchFilterIndexError,
        > {
            Ok(Default::default())
        }
    }

    struct BlockingIndex {
        filters: Vec<SearchFilterView>,
        percolation_started: Arc<Notify>,
        resume_percolation: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl SearchFilterIndex for BlockingIndex {
        async fn upsert(
            &self,
            _projection: &crate::ports::SearchFilterProjection,
        ) -> Result<SearchFilterProjectionWriteOutcome, SearchFilterIndexError> {
            Ok(SearchFilterProjectionWriteOutcome::Applied)
        }

        async fn delete(
            &self,
            _id: UserSearchFilterId,
            _source_version: i64,
        ) -> Result<SearchFilterProjectionWriteOutcome, SearchFilterIndexError> {
            Ok(SearchFilterProjectionWriteOutcome::Applied)
        }

        async fn percolate(
            &self,
            _input: &ProductPercolationInput,
        ) -> Result<Vec<SearchFilterView>, SearchFilterIndexError> {
            self.percolation_started.notify_one();
            self.resume_percolation.notified().await;
            Ok(self.filters.clone())
        }

        async fn query(
            &self,
            _query: &SearchFilterIndexQuery,
        ) -> Result<
            common::pagination::cursor::CursoredResult<SearchFilterView, serde_json::Value>,
            SearchFilterIndexError,
        > {
            Ok(Default::default())
        }
    }

    #[derive(Clone)]
    struct FxRates(Arc<Mutex<State>>);

    struct ReadingFxRates<'a>(&'a Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl FxRateSnapshotRepository for ReadingFxRates<'_> {
        async fn find_latest(
            &mut self,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(None)
        }

        async fn find_latest_at_or_before(
            &mut self,
            _timestamp: OffsetDateTime,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            let mut state =
                self.0
                    .lock()
                    .map_err(|_| FxRateSnapshotRepositoryError::ReadFailed {
                        source: box_error(std::io::Error::other("test mutex poisoned")),
                    })?;
            state.event_snapshot_reads += 1;
            Ok(state.event_snapshot.clone())
        }

        async fn find_by_id(
            &mut self,
            _id: FxRateId,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            let mut state =
                self.0
                    .lock()
                    .map_err(|_| FxRateSnapshotRepositoryError::ReadFailed {
                        source: box_error(std::io::Error::other("test mutex poisoned")),
                    })?;
            state.sale_snapshot_reads += 1;
            Ok(state.sale_snapshot.clone())
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

    impl FxRateSnapshotRepositoryFactory<FakeTransaction> for FxRates {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl FxRateSnapshotRepository + 'tx {
            ReadingFxRates(&self.0)
        }
    }

    struct Evaluator;
    struct PermanentlyFailingEvaluator;

    #[async_trait::async_trait]
    impl LargeLanguageModel for Evaluator {
        async fn generate<Output>(
            &self,
            _request: StructuredGenerationRequest,
        ) -> Result<Output, LargeLanguageModelError>
        where
            Output: serde::de::DeserializeOwned + Send,
        {
            serde_json::from_str(r#"{"matches":false}"#).map_err(|source| {
                LargeLanguageModelError::InvalidResponse {
                    source: box_error(source),
                }
            })
        }
    }

    #[async_trait::async_trait]
    impl LargeLanguageModel for PermanentlyFailingEvaluator {
        async fn generate<Output>(
            &self,
            _request: StructuredGenerationRequest,
        ) -> Result<Output, LargeLanguageModelError>
        where
            Output: serde::de::DeserializeOwned + Send,
        {
            Err(LargeLanguageModelError::Permanent {
                source: box_error(std::io::Error::other("invalid Vertex request")),
            })
        }
    }

    #[derive(Clone)]
    struct Candidates(Arc<Mutex<State>>);

    struct ReadingActiveCandidates<'a>(&'a Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl ActiveSearchFilterMatchCandidateReader for ReadingActiveCandidates<'_> {
        async fn find_active(
            &mut self,
            candidates: &[SearchFilterMatchCandidate],
        ) -> Result<Vec<ActiveSearchFilterMatchCandidate>, ActiveSearchFilterMatchCandidateReadError>
        {
            self.0
                .lock()
                .map_err(|_| ActiveSearchFilterMatchCandidateReadError::ReadFailed {
                    source: box_error(std::io::Error::other("test mutex poisoned")),
                })?
                .active_reads += 1;
            Ok(candidates
                .iter()
                .map(|candidate| ActiveSearchFilterMatchCandidate {
                    user_id: candidate.user_id,
                    search_filter_id: candidate.search_filter_id,
                    search_filter_name: UserSearchFilterName::from(
                        candidate.search_filter_id.to_string(),
                    ),
                    price_match_valuation: candidate.price_match_valuation,
                    enhanced_match_reason: candidate.enhanced_match_reason.clone(),
                })
                .collect())
        }
    }

    impl ActiveSearchFilterMatchCandidateReaderFactory<FakeTransaction> for Candidates {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ActiveSearchFilterMatchCandidateReader + 'tx {
            ReadingActiveCandidates(&self.0)
        }
    }

    #[derive(Clone)]
    struct Matches(Arc<Mutex<State>>);

    struct WritingMatches<'a>(&'a Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl SearchFilterMatchWriter for WritingMatches<'_> {
        async fn insert_if_absent(
            &mut self,
            product_match: &SearchFilterProductMatch,
        ) -> Result<SearchFilterMatchPersistOutcome, SearchFilterMatchWriteError> {
            let mut state =
                self.0
                    .lock()
                    .map_err(|_| SearchFilterMatchWriteError::WriteFailed {
                        source: box_error(std::io::Error::other("test mutex poisoned")),
                    })?;
            if state.persisted.iter().any(|persisted| {
                persisted.user_search_filter_id == product_match.user_search_filter_id
                    && persisted.product_id == product_match.product_id
            }) {
                return Ok(SearchFilterMatchPersistOutcome::AlreadyExists);
            }
            state.persisted.push(product_match.clone());
            Ok(SearchFilterMatchPersistOutcome::Inserted)
        }
    }

    impl SearchFilterMatchWriterFactory<FakeTransaction> for Matches {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl SearchFilterMatchWriter + 'tx {
            WritingMatches(&self.0)
        }
    }

    fn product() -> Result<ProductSearchFilterMatchSource, url::ParseError> {
        let url = Url::parse("https://example.test/product")?;
        let event_id = EventId::new();
        Ok(ProductSearchFilterMatchSource {
            event_id,
            event_kind: ProductSearchFilterMatchSourceEventKind::Domain,
            origin_event_time: OffsetDateTime::UNIX_EPOCH,
            current_event_id: event_id,
            projection_version: 1,
            product_id: common::product_id::ProductId::new(),
            product_slug_id: ProductSlugId::from("product"),
            shop_id: ShopId::new(),
            shop_slug_id: ShopSlugId::from("shop"),
            shop_name: ShopName::from("Shop"),
            shop_type: ProductSearchFilterMatchShopType::Marketplace,
            seller_id: ShopId::new(),
            seller_slug_id: common::seller_slug_id::SellerSlugId::from("seller"),
            seller_name: ShopName::from("Seller"),
            shops_product_id: ShopsProductId::from("product"),
            address: ProductAddress::default(),
            product_title: None,
            product_description: None,
            titles: std::collections::HashMap::new(),
            descriptions: std::collections::HashMap::new(),
            pricing: ProductPricing::default(),
            sale_valuation: None,
            state: ProductState::Available,
            lifecycle: ProductLifecycle::Active,
            url: url.clone(),
            view_url: url,
            image: None,
            images: IndexSet::<ProductImage>::new(),
            embedding: None,
            auction: ProductAuction::default(),
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        })
    }

    fn fx_snapshot(generation: i64, captured_at: OffsetDateTime) -> FxRateSnapshot {
        let quotes = Currency::iter().map(|currency| {
            FxRateQuote::new(
                currency,
                match currency {
                    Currency::Eur => FX_RATE_SCALE,
                    Currency::Gbp => 850_000,
                    Currency::Usd => 1_100_000,
                    Currency::Jpy => 160_000_000,
                    _ => 1_250_000,
                },
            )
        });
        NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            captured_at,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            quotes,
        )
        .unwrap_or_else(|error| panic!("valid snapshot: {error}"))
        .into_persisted(
            FxRateGeneration::try_from(generation)
                .unwrap_or_else(|error| panic!("valid generation: {error}")),
        )
    }

    fn filter(user_id: UserId, search_filter_id: UserSearchFilterId) -> SearchFilterView {
        SearchFilterView {
            search_filter_id,
            user_id,
            name: UserSearchFilterName::from("daily"),
            notifications: true,
            state: ResourceState::Active,
            search: search_filter_core::ProductSearch::new(Language::En, Currency::Eur),
            embedding: None,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
            last_hybrid_search_matched: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn matching_handler(
        state: Arc<Mutex<State>>,
        sources: Vec<ProductSearchFilterMatchSource>,
        search_filter: SearchFilterView,
    ) -> MatchProductEventHandler<
        FakeUnitOfWork,
        Sources,
        Revisions,
        FxRates,
        Index,
        Evaluator,
        Candidates,
        Matches,
    > {
        MatchProductEventHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            Sources(sources),
            Revisions(Arc::clone(&state)),
            FxRates(Arc::clone(&state)),
            Index {
                filters: vec![search_filter],
            },
            Evaluator,
            Candidates(Arc::clone(&state)),
            Matches(state),
        )
    }

    #[tokio::test]
    async fn should_persist_all_active_candidates_without_a_notification_quota()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let user_id = UserId::new();
        let product = product()?;
        let handler = MatchProductEventHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            Sources(vec![product.clone()]),
            Revisions(Arc::clone(&state)),
            FxRates(Arc::clone(&state)),
            Index {
                filters: vec![
                    filter(user_id, UserSearchFilterId::new()),
                    filter(user_id, UserSearchFilterId::new()),
                ],
            },
            Evaluator,
            Candidates(Arc::clone(&state)),
            Matches(Arc::clone(&state)),
        );

        let result = handler
            .execute(MatchProductEventCommand {
                origin_event_id: product.event_id,
                product_id: product.product_id,
            })
            .await?;

        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(2, result.percolated_count);
        assert_eq!(2, result.persisted_match_count);
        assert_eq!(2, state.committed);
        assert_eq!(1, state.active_reads);
        assert_eq!(0, state.sale_snapshot_reads);
        assert_eq!(2, state.persisted.len());
        Ok(())
    }

    #[tokio::test]
    async fn should_use_event_time_snapshot_and_persist_price_match_provenance()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let event_snapshot = fx_snapshot(1, OffsetDateTime::UNIX_EPOCH);
        state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?
            .event_snapshot = Some(event_snapshot.clone());
        let mut source = product()?;
        source.pricing.price = Some(Price::new(MonetaryAmount::from(12_500_u64), Currency::Gbp));
        let mut saved_filter = filter(UserId::new(), UserSearchFilterId::new());
        saved_filter.search.price_query = Some(RangeQuery {
            min: Some(MonetaryAmount::from(10_000_u64)),
            max: Some(MonetaryAmount::from(20_000_u64)),
        });
        let handler = matching_handler(Arc::clone(&state), vec![source.clone()], saved_filter);

        let result = handler
            .execute(MatchProductEventCommand {
                origin_event_id: source.event_id,
                product_id: source.product_id,
            })
            .await?;

        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(MatchProductEventOutcome::Processed, result.outcome);
        assert_eq!(1, state.event_snapshot_reads);
        assert_eq!(0, state.sale_snapshot_reads);
        assert_eq!(
            Some(PriceMatchValuation {
                basis: ProductPriceValuationBasis::Event,
                fx_rate_id: event_snapshot.id(),
            }),
            state.persisted[0].price_match_valuation
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_use_immutable_sale_snapshot_instead_of_newer_event_snapshot()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let sale_snapshot = fx_snapshot(1, OffsetDateTime::UNIX_EPOCH);
        let newer_event_snapshot =
            fx_snapshot(2, OffsetDateTime::UNIX_EPOCH + time::Duration::days(1));
        {
            let mut state = state
                .lock()
                .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
            state.sale_snapshot = Some(sale_snapshot.clone());
            state.event_snapshot = Some(newer_event_snapshot);
        }
        let mut source = product()?;
        source.pricing.price = Some(Price::new(MonetaryAmount::from(12_500_u64), Currency::Gbp));
        source.sale_valuation = Some(ProductSaleValuation {
            fx_rate_id: sale_snapshot.id(),
            sold_at: OffsetDateTime::UNIX_EPOCH,
        });
        source.state = ProductState::Sold;
        let mut saved_filter = filter(UserId::new(), UserSearchFilterId::new());
        saved_filter.search.price_query = Some(RangeQuery {
            min: Some(MonetaryAmount::from(1_u64)),
            max: Some(MonetaryAmount::from(1_000_000_u64)),
        });
        let handler = matching_handler(Arc::clone(&state), vec![source.clone()], saved_filter);

        handler
            .execute(MatchProductEventCommand {
                origin_event_id: source.event_id,
                product_id: source.product_id,
            })
            .await?;

        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(1, state.sale_snapshot_reads);
        assert_eq!(0, state.event_snapshot_reads);
        assert_eq!(
            Some(PriceMatchValuation {
                basis: ProductPriceValuationBasis::Sale,
                fx_rate_id: sale_snapshot.id(),
            }),
            state.persisted[0].price_match_valuation
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_fail_when_event_effective_snapshot_is_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut source = product()?;
        source.pricing.price = Some(Price::new(MonetaryAmount::from(1_u64), Currency::Eur));
        let handler = matching_handler(
            Arc::new(Mutex::new(State::default())),
            vec![source.clone()],
            filter(UserId::new(), UserSearchFilterId::new()),
        );

        let error = handler
            .execute(MatchProductEventCommand {
                origin_event_id: source.event_id,
                product_id: source.product_id,
            })
            .await
            .err()
            .ok_or_else(|| std::io::Error::other("missing snapshot must fail"))?;
        assert!(matches!(
            error,
            MatchProductEventError::EventSnapshotNotFound { .. }
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_fail_when_sale_snapshot_is_missing() -> Result<(), Box<dyn std::error::Error>> {
        let mut source = product()?;
        source.pricing.price = Some(Price::new(MonetaryAmount::from(1_u64), Currency::Eur));
        source.sale_valuation = Some(ProductSaleValuation {
            fx_rate_id: FxRateId::new(),
            sold_at: OffsetDateTime::UNIX_EPOCH,
        });
        source.state = ProductState::Sold;
        let handler = matching_handler(
            Arc::new(Mutex::new(State::default())),
            vec![source.clone()],
            filter(UserId::new(), UserSearchFilterId::new()),
        );

        let error = handler
            .execute(MatchProductEventCommand {
                origin_event_id: source.event_id,
                product_id: source.product_id,
            })
            .await
            .err()
            .ok_or_else(|| std::io::Error::other("missing sale snapshot must fail"))?;
        assert!(matches!(
            error,
            MatchProductEventError::SaleSnapshotNotFound { .. }
        ));
        Ok(())
    }

    #[test]
    fn should_include_only_the_first_five_product_images_in_an_enhanced_request()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut product = product()?;
        let image_urls = (0..7)
            .map(|index| Url::parse(&format!("https://example.test/image-{index}.jpg")))
            .collect::<Result<Vec<_>, _>>()?;
        for url in &image_urls {
            product.images.insert(ProductImage {
                url: url.clone(),
                prohibited_content: product_core::prohibited_content::ProhibitedContent::None,
            });
        }
        let mut enhanced = filter(UserId::new(), UserSearchFilterId::new());
        enhanced.search.enhanced_search_description = Some("only paintings".into());

        let request = enhanced_filter_request(&product, &enhanced)?;

        assert_eq!(
            &image_urls[..MAX_PRODUCT_MATCH_IMAGES],
            request.image_urls.as_slice()
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_persist_plain_candidate_when_enhanced_candidate_fails_permanently()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let user_id = UserId::new();
        let product = product()?;
        let plain = filter(user_id, UserSearchFilterId::new());
        let mut enhanced = filter(user_id, UserSearchFilterId::new());
        enhanced.search.enhanced_search_description = Some("only paintings".into());
        let handler = MatchProductEventHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            Sources(vec![product.clone()]),
            Revisions(Arc::clone(&state)),
            FxRates(Arc::clone(&state)),
            Index {
                filters: vec![plain.clone(), enhanced],
            },
            PermanentlyFailingEvaluator,
            Candidates(Arc::clone(&state)),
            Matches(Arc::clone(&state)),
        );

        let result = handler
            .execute(MatchProductEventCommand {
                origin_event_id: product.event_id,
                product_id: product.product_id,
            })
            .await?;

        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(1, result.persisted_match_count);
        assert_eq!(1, result.enhanced_evaluation_failure_count);
        assert_eq!(1, state.persisted.len());
        assert_eq!(
            plain.search_filter_id,
            state.persisted[0].user_search_filter_id
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_skip_enhanced_candidate_when_evaluator_does_not_match()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let user_id = UserId::new();
        let mut enhanced = filter(user_id, UserSearchFilterId::new());
        enhanced.search.enhanced_search_description = Some("only paintings".into());
        let product = product()?;
        let handler = MatchProductEventHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            Sources(vec![product.clone()]),
            Revisions(Arc::clone(&state)),
            FxRates(Arc::clone(&state)),
            Index {
                filters: vec![enhanced],
            },
            Evaluator,
            Candidates(Arc::clone(&state)),
            Matches(Arc::clone(&state)),
        );

        let result = handler
            .execute(MatchProductEventCommand {
                origin_event_id: product.event_id,
                product_id: product.product_id,
            })
            .await?;

        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(1, result.percolated_count);
        assert_eq!(0, result.persisted_match_count);
        assert_eq!(2, state.committed);
        assert_eq!(0, state.active_reads);
        Ok(())
    }

    #[tokio::test]
    async fn should_keep_matches_order_invariant_for_domain_and_enrichment_events()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let search_filter_id = UserSearchFilterId::new();
        let mut event_a = product()?;
        let mut event_b = event_a.clone();
        event_b.event_id = EventId::new();
        event_b.current_event_id = event_b.event_id;
        event_b.event_kind = ProductSearchFilterMatchSourceEventKind::Enrichment;
        event_a.current_event_id = event_b.event_id;

        let state = Arc::new(Mutex::new(State::default()));
        let handler = matching_handler(
            Arc::clone(&state),
            vec![event_a.clone(), event_b.clone()],
            filter(user_id, search_filter_id),
        );
        let stale_first = handler
            .execute(MatchProductEventCommand {
                origin_event_id: event_a.event_id,
                product_id: event_a.product_id,
            })
            .await?;
        let current_second = handler
            .execute(MatchProductEventCommand {
                origin_event_id: event_b.event_id,
                product_id: event_b.product_id,
            })
            .await?;
        let redelivered_stale = handler
            .execute(MatchProductEventCommand {
                origin_event_id: event_a.event_id,
                product_id: event_a.product_id,
            })
            .await?;
        let redelivered_current = handler
            .execute(MatchProductEventCommand {
                origin_event_id: event_b.event_id,
                product_id: event_b.product_id,
            })
            .await?;

        assert_eq!(
            MatchProductEventOutcome::StaleSourceSkipped,
            stale_first.outcome
        );
        assert_eq!(MatchProductEventOutcome::Processed, current_second.outcome);
        assert_eq!(
            MatchProductEventOutcome::StaleSourceSkipped,
            redelivered_stale.outcome
        );
        assert_eq!(
            MatchProductEventOutcome::DuplicateAlreadyPersisted,
            redelivered_current.outcome
        );
        assert_eq!(
            vec![event_b.event_id],
            state
                .lock()
                .map_err(|_| std::io::Error::other("test mutex poisoned"))?
                .persisted
                .iter()
                .map(|persisted| persisted.origin_event_id)
                .collect::<Vec<_>>()
        );

        let reverse_state = Arc::new(Mutex::new(State::default()));
        let reverse_handler = matching_handler(
            Arc::clone(&reverse_state),
            vec![event_a.clone(), event_b.clone()],
            filter(user_id, search_filter_id),
        );
        let current_first = reverse_handler
            .execute(MatchProductEventCommand {
                origin_event_id: event_b.event_id,
                product_id: event_b.product_id,
            })
            .await?;
        let stale_second = reverse_handler
            .execute(MatchProductEventCommand {
                origin_event_id: event_a.event_id,
                product_id: event_a.product_id,
            })
            .await?;

        assert_eq!(MatchProductEventOutcome::Processed, current_first.outcome);
        assert_eq!(
            MatchProductEventOutcome::StaleSourceSkipped,
            stale_second.outcome
        );
        assert_eq!(
            vec![event_b.event_id],
            reverse_state
                .lock()
                .map_err(|_| std::io::Error::other("test mutex poisoned"))?
                .persisted
                .iter()
                .map(|persisted| persisted.origin_event_id)
                .collect::<Vec<_>>()
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_distinguish_ignored_and_missing_product_sources()
    -> Result<(), Box<dyn std::error::Error>> {
        let ignored_state = Arc::new(Mutex::new(State::default()));
        let mut ignored_product = product()?;
        ignored_product.event_kind = ProductSearchFilterMatchSourceEventKind::Ignored;
        let ignored = matching_handler(
            Arc::clone(&ignored_state),
            vec![ignored_product.clone()],
            filter(UserId::new(), UserSearchFilterId::new()),
        )
        .execute(MatchProductEventCommand {
            origin_event_id: ignored_product.event_id,
            product_id: ignored_product.product_id,
        })
        .await?;
        let missing = matching_handler(
            Arc::new(Mutex::new(State::default())),
            Vec::new(),
            filter(UserId::new(), UserSearchFilterId::new()),
        )
        .execute(MatchProductEventCommand {
            origin_event_id: EventId::new(),
            product_id: ignored_product.product_id,
        })
        .await?;

        assert_eq!(MatchProductEventOutcome::IgnoredEventType, ignored.outcome);
        assert_eq!(MatchProductEventOutcome::SourceNotFound, missing.outcome);
        Ok(())
    }

    #[tokio::test]
    async fn should_skip_stale_event_when_product_advances_during_percolation()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let product = product()?;
        let percolation_started = Arc::new(Notify::new());
        let resume_percolation = Arc::new(Notify::new());
        let handler = MatchProductEventHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            Sources(vec![product.clone()]),
            Revisions(Arc::clone(&state)),
            FxRates(Arc::clone(&state)),
            BlockingIndex {
                filters: vec![filter(UserId::new(), UserSearchFilterId::new())],
                percolation_started: Arc::clone(&percolation_started),
                resume_percolation: Arc::clone(&resume_percolation),
            },
            Evaluator,
            Candidates(Arc::clone(&state)),
            Matches(Arc::clone(&state)),
        );

        let product_id = product.product_id;
        let event_id = product.event_id;
        let matching = tokio::spawn(async move {
            handler
                .execute(MatchProductEventCommand {
                    origin_event_id: event_id,
                    product_id,
                })
                .await
        });
        percolation_started.notified().await;
        state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?
            .current_event_id = Some(EventId::new());
        resume_percolation.notify_one();

        let result = matching.await??;
        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(MatchProductEventOutcome::StaleSourceSkipped, result.outcome);
        assert_eq!(0, result.persisted_match_count);
        assert_eq!(0, state.active_reads);
        assert!(state.persisted.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_ignore_stale_product_events_after_committing_source_read()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let mut product = product()?;
        product.current_event_id = EventId::new();
        let handler = MatchProductEventHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            Sources(vec![product.clone()]),
            Revisions(Arc::clone(&state)),
            FxRates(Arc::clone(&state)),
            Index {
                filters: Vec::new(),
            },
            Evaluator,
            Candidates(Arc::clone(&state)),
            Matches(Arc::clone(&state)),
        );

        let result = handler
            .execute(MatchProductEventCommand {
                origin_event_id: product.event_id,
                product_id: product.product_id,
            })
            .await?;

        assert_eq!(MatchProductEventOutcome::StaleSourceSkipped, result.outcome);
        assert_eq!(
            1,
            state
                .lock()
                .map_err(|_| std::io::Error::other("test mutex poisoned"))?
                .committed
        );
        Ok(())
    }
}
