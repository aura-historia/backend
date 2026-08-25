use crate::ports::{
    CompiledProductListingSearch, ProductListingPriceFilterPlan, ProductListingSearchReadError,
    ProductListingSearchReadRequest, ProductListingSearchReader, ProductListingUserStateReader,
};
use crate::use_cases::queries::product_listing_summary_personalization::{
    ProductListingSummaryPersonalizationError, hydrate_product_summaries,
};
use application::error::{BoxError, box_error};
use application::operation_context::{OperationContext, Principal};
use application::pagination::{Cursor, CursoredResult};
use application::personalized::Personalized;
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use domain_primitives::sort::Sort;
use embedding::{EmbeddingGenerator, EmbeddingText};
use fxrate_core::{FxRateId, FxRateSnapshot, FxRateSnapshotError};
use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use localization::Language;
use localization::Localized;
use money::Price;
use product_listing_core::product_lifecycle::ProductLifecycle;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_core::product_state::ProductState;
use product_listing_core::shop_listing_id::ShopListingId;
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use shop_core::shop_slug_id::ShopSlugId;

use crate::user_state::ProductListingUserState;
use indexmap::IndexSet;

use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::product_listing_search::ProductListingSearch;
use product_listing_core::sort_product_listing_field::SortProductListingField;
use product_listing_core::title::Title;
use serde_json::Value;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchProductListingsRequest {
    pub search: ProductListingSearch,
    pub sort: Option<Sort<SortProductListingField>>,
    pub cursor: Option<Cursor<ProductListingSearchCursor>>,
}

/// Opaque ProductListing-owned continuation state.
///
/// The OpenSearch sort token is scoped to one immutable persisted FX snapshot, so active
/// ProductListing presentation and price-range membership cannot change within a cursor chain.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingSearchCursor {
    pub fx_rate_id: FxRateId,
    pub search_after: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingSummary {
    pub product_id: ProductListingId,
    pub product_slug_id: ProductListingSlugId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shop_listing_id: ShopListingId,
    pub shop_name: ShopName,
    pub shop_slug_id: ShopSlugId,
    pub title: Option<Localized<Language, Title>>,
    pub display_price: Option<Price>,
    pub price_valuation: ProductListingSummaryPriceValuation,
    pub state: ProductState,
    pub lifecycle: ProductLifecycle,
    pub url: Url,
    pub view_url: Url,
    pub images: IndexSet<ProductListingImage>,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductListingSummaryPriceValuation {
    Current {
        fx_rate_id: FxRateId,
        captured_at: OffsetDateTime,
    },
    Sale {
        fx_rate_id: FxRateId,
        sold_at: OffsetDateTime,
    },
}

pub type PersonalizedProductListingSummary =
    Personalized<ProductListingSummary, ProductListingUserState>;
pub type ProductListingSearchReadResult = CursoredResult<ProductListingSummary, Value>;
pub type SearchProductListingsResult =
    CursoredResult<PersonalizedProductListingSummary, ProductListingSearchCursor>;

#[derive(Debug, thiserror::Error)]
pub enum SearchProductListingsError {
    #[error("product search query failed")]
    ProductListingSearchQueryFailed,
    #[error("product search read model is invalid")]
    ProductListingSearchReadModelInvalid,
    #[error("pinned FX rate snapshot is missing")]
    FxRateSnapshotMissing,
    #[error("failed to begin FX rate snapshot transaction")]
    BeginFxRateSnapshotTransactionFailed {
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
    #[error("failed to commit FX rate snapshot transaction")]
    CommitFxRateSnapshotTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("product user state query failed")]
    ProductListingUserStateQueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("product user state read model is invalid")]
    ProductListingUserStateReadModelInvalid {
        #[source]
        source: BoxError,
    },

    #[error("product user state is missing")]
    ProductListingUserStateMissing,
    #[error("hidden product summary could not be constructed")]
    HiddenProductListingSummaryInvalid {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait SearchProductListingsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchProductListingsRequest,
    ) -> Result<SearchProductListingsResult, SearchProductListingsError>;
}

pub struct SearchProductListingsHandler<UoW, R, F, E, U> {
    unit_of_work: UoW,
    reader: R,
    fx_rates: F,
    embeddings: E,
    user_states: U,
}

impl<UoW, R, F, E, U> SearchProductListingsHandler<UoW, R, F, E, U> {
    pub fn new(unit_of_work: UoW, reader: R, fx_rates: F, embeddings: E, user_states: U) -> Self {
        Self {
            unit_of_work,
            reader,
            fx_rates,
            embeddings,
            user_states,
        }
    }
}

#[async_trait::async_trait]
impl<UoW, R, F, E, U> SearchProductListingsUseCase for SearchProductListingsHandler<UoW, R, F, E, U>
where
    UoW: UnitOfWork,
    R: ProductListingSearchReader,
    F: FxRateSnapshotRepositoryFactory<UoW::Tx>,
    E: EmbeddingGenerator,
    U: ProductListingUserStateReader,
{
    #[tracing::instrument(
        name = "search_products",
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
        request: SearchProductListingsRequest,
    ) -> Result<SearchProductListingsResult, SearchProductListingsError> {
        let valuation_at = OffsetDateTime::now_utc();
        let pinned_fx_rate_id = request.cursor.as_ref().and_then(|cursor| {
            cursor
                .search_after
                .as_ref()
                .map(|search_after| search_after.fx_rate_id)
        });
        let snapshot = load_fx_rate_snapshot(
            &self.unit_of_work,
            &self.fx_rates,
            pinned_fx_rate_id,
            valuation_at,
        )
        .await?;
        let price_filter = compile_price_filter(snapshot, &request)?;
        let fx_rate_id = price_filter.fx_rate_id;
        let embedding_query = hybrid_embedding_query(&request);
        let read_request = ProductListingSearchReadRequest {
            compiled_search: CompiledProductListingSearch {
                search: request.search,
                price_filter_plan: price_filter,
            },
            sort: request.sort,
            cursor: request.cursor.map(|cursor| Cursor {
                size: cursor.size,
                search_after: cursor.search_after.map(|value| value.search_after),
            }),
        };
        let result = match embedding_query {
            Some(query) => match self.embeddings.embed_search_query(&query).await {
                Ok(embedding) => {
                    self.reader
                        .search_hybrid(&read_request, embedding.values())
                        .await?
                }
                Err(_) => self.reader.search(&read_request).await?,
            },
            None => self.reader.search(&read_request).await?,
        };
        let mut result = CursoredResult {
            cursor: Cursor {
                size: result.cursor.size,
                search_after: result.cursor.search_after.map(|search_after| {
                    ProductListingSearchCursor {
                        fx_rate_id,
                        search_after,
                    }
                }),
            },
            items: result
                .items
                .into_iter()
                .map(|item| Personalized {
                    item,
                    user_state: None,
                })
                .collect(),
            total: result.total,
        };
        if let Some(user_id) = personalization_user_id(&context.principal) {
            hydrate_product_summaries(&mut result.items, user_id, &self.user_states).await?;
        }
        Ok(result)
    }
}

async fn load_fx_rate_snapshot<UoW, F>(
    unit_of_work: &UoW,
    fx_rates: &F,
    pinned_fx_rate_id: Option<FxRateId>,
    valuation_at: OffsetDateTime,
) -> Result<FxRateSnapshot, SearchProductListingsError>
where
    UoW: UnitOfWork,
    F: FxRateSnapshotRepositoryFactory<UoW::Tx>,
{
    let mut tx = unit_of_work.begin().await.map_err(|source| {
        SearchProductListingsError::BeginFxRateSnapshotTransactionFailed {
            source: box_error(source),
        }
    })?;
    let snapshot = match pinned_fx_rate_id {
        Some(fx_rate_id) => fx_rates
            .in_transaction(&mut tx)
            .find_by_id(fx_rate_id)
            .await
            .map_err(fx_rate_snapshot_read_error)?,
        None => fx_rates
            .in_transaction(&mut tx)
            .find_latest_at_or_before(valuation_at)
            .await
            .map_err(fx_rate_snapshot_read_error)?,
    };
    tx.commit().await.map_err(|source| {
        SearchProductListingsError::CommitFxRateSnapshotTransactionFailed {
            source: box_error(source),
        }
    })?;
    snapshot.ok_or(SearchProductListingsError::FxRateSnapshotMissing)
}

fn compile_price_filter(
    snapshot: FxRateSnapshot,
    request: &SearchProductListingsRequest,
) -> Result<ProductListingPriceFilterPlan, SearchProductListingsError> {
    ProductListingPriceFilterPlan::compile(
        snapshot,
        request.search.currency,
        request.search.price_query,
    )
    .map_err(
        |error: FxRateSnapshotError| SearchProductListingsError::FxRateSnapshotInvalid {
            source: box_error(error),
        },
    )
}

fn fx_rate_snapshot_read_error(error: FxRateSnapshotRepositoryError) -> SearchProductListingsError {
    match error {
        FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { source } => {
            SearchProductListingsError::FxRateSnapshotInvalid { source }
        }
        FxRateSnapshotRepositoryError::InsertFailed { source }
        | FxRateSnapshotRepositoryError::ReadFailed { source } => {
            SearchProductListingsError::FxRateSnapshotReadFailed { source }
        }
        FxRateSnapshotRepositoryError::CapturedAtNotMonotonic => {
            SearchProductListingsError::FxRateSnapshotMissing
        }
    }
}

fn hybrid_embedding_query(request: &SearchProductListingsRequest) -> Option<EmbeddingText> {
    if !matches!(
        request.sort.as_ref().map(|sort| sort.sort),
        None | Some(SortProductListingField::Score)
    ) {
        return None;
    }

    let text = request
        .search
        .product_listing_query
        .iter()
        .map(AsRef::as_ref)
        .filter(|query: &&str| !query.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    let Ok(text) = EmbeddingText::new(text) else {
        return None;
    };

    Some(text)
}

fn personalization_user_id(principal: &Principal) -> Option<user_core::user_id::UserId> {
    match principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}

impl From<ProductListingSearchReadError> for SearchProductListingsError {
    fn from(error: ProductListingSearchReadError) -> Self {
        match error {
            ProductListingSearchReadError::ProductListingSearchQueryFailed => {
                Self::ProductListingSearchQueryFailed
            }
            ProductListingSearchReadError::ProductListingSearchReadModelInvalid => {
                Self::ProductListingSearchReadModelInvalid
            }
        }
    }
}

impl From<ProductListingSummaryPersonalizationError> for SearchProductListingsError {
    fn from(error: ProductListingSummaryPersonalizationError) -> Self {
        match error {
            ProductListingSummaryPersonalizationError::UserStateQueryFailed { source } => {
                Self::ProductListingUserStateQueryFailed { source }
            }
            ProductListingSummaryPersonalizationError::UserStateReadModelInvalid { source } => {
                Self::ProductListingUserStateReadModelInvalid { source }
            }

            ProductListingSummaryPersonalizationError::UserStateMissing { .. } => {
                Self::ProductListingUserStateMissing
            }
            ProductListingSummaryPersonalizationError::HiddenProductListingSummaryInvalid {
                source,
            } => Self::HiddenProductListingSummaryInvalid { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{ProductListingUserStateLookup, ProductListingUserStateReadError};
    use application::error::box_error;
    use application::operation_context::{CorrelationId, Principal, RequestId};
    use application::transaction::{TransactionError, UnitOfWork};
    use domain_primitives::event_id::EventId;
    use embedding::{EmbeddingError, EmbeddingVector};
    use fxrate_core::{
        FX_RATE_SCALE, FxRateId, FxRateQuote, FxRateSnapshot, FxRateSource, NewFxRateSnapshot,
    };
    use fxrate_service::ports::FxRateSnapshotRepositoryFactory;
    use localization::Language;
    use money::Currency;
    use money::MonetaryAmount;
    use user_core::user_id::UserId;

    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, MutexGuard};
    use strum::IntoEnumIterator;

    #[derive(Debug, Default)]
    struct FakeState {
        search_result:
            Option<Result<ProductListingSearchReadResult, ProductListingSearchReadError>>,
        hybrid_search_result:
            Option<Result<ProductListingSearchReadResult, ProductListingSearchReadError>>,
        read_requests: Vec<ProductListingSearchReadRequest>,
        reader_observed_commit_counts: Vec<usize>,
        begin_error: bool,
        commit_error: bool,
        commit_count: usize,
        fx_rate_snapshot: Option<Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError>>,
        fx_rate_snapshot_by_id:
            Option<Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError>>,
        embedding_result: Option<Result<EmbeddingVector, EmbeddingError>>,
        embedding_queries: Vec<String>,
        used_hybrid_search: bool,
        user_states_result: Option<
            Result<
                HashMap<ProductListingId, ProductListingUserState>,
                ProductListingUserStateReadError,
            >,
        >,
        user_state_lookups: Vec<ProductListingUserStateLookup>,
    }

    type SharedState = Arc<Mutex<FakeState>>;

    #[derive(Clone)]
    struct FakeSearchReader {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeUnitOfWork {
        state: SharedState,
    }

    struct FakeTx {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeFxRateSnapshotRepositoryFactory {
        state: SharedState,
    }

    struct FakeFxRateSnapshotRepository {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeUserStatesReader {
        state: SharedState,
    }

    fn state() -> SharedState {
        Arc::new(Mutex::new(FakeState::default()))
    }

    fn lock_state(state: &SharedState) -> MutexGuard<'_, FakeState> {
        match state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn search_reader(state: &SharedState) -> FakeSearchReader {
        FakeSearchReader {
            state: Arc::clone(state),
        }
    }

    #[async_trait::async_trait]
    impl ProductListingSearchReader for FakeSearchReader {
        async fn search(
            &self,
            request: &ProductListingSearchReadRequest,
        ) -> Result<ProductListingSearchReadResult, ProductListingSearchReadError> {
            let mut state = lock_state(&self.state);
            state.read_requests.push(request.clone());
            let commit_count = state.commit_count;
            state.reader_observed_commit_counts.push(commit_count);
            match state.search_result.take() {
                Some(result) => result,
                None => Ok(CursoredResult::default()),
            }
        }

        async fn search_hybrid(
            &self,
            request: &ProductListingSearchReadRequest,
            _embedding: &[f32],
        ) -> Result<ProductListingSearchReadResult, ProductListingSearchReadError> {
            let mut state = lock_state(&self.state);
            state.read_requests.push(request.clone());
            let commit_count = state.commit_count;
            state.reader_observed_commit_counts.push(commit_count);
            state.used_hybrid_search = true;
            match state.hybrid_search_result.take() {
                Some(result) => result,
                None => Ok(CursoredResult::default()),
            }
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            if lock_state(&self.state).begin_error {
                return Err(TransactionError::BeginFailed);
            }
            Ok(FakeTx {
                state: Arc::clone(&self.state),
            })
        }
    }

    #[async_trait::async_trait]
    impl application::transaction::Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = lock_state(&self.state);
            state.commit_count += 1;
            if state.commit_error {
                Err(TransactionError::CommitFailed)
            } else {
                Ok(())
            }
        }
    }

    impl FxRateSnapshotRepositoryFactory<FakeTx> for FakeFxRateSnapshotRepositoryFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl FxRateSnapshotRepository + 'tx {
            FakeFxRateSnapshotRepository {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl FxRateSnapshotRepository for FakeFxRateSnapshotRepository {
        async fn find_latest(
            &mut self,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            let mut state = lock_state(&self.state);
            match state.fx_rate_snapshot.take() {
                Some(result) => result,
                None => snapshot().map(Some).map_err(|source| {
                    FxRateSnapshotRepositoryError::InvalidPersistedSnapshot {
                        source: box_error(source),
                    }
                }),
            }
        }

        async fn find_latest_at_or_before(
            &mut self,
            _timestamp: OffsetDateTime,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            self.find_latest().await
        }

        async fn find_by_id(
            &mut self,
            _id: FxRateId,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            let mut state = lock_state(&self.state);
            match state.fx_rate_snapshot_by_id.take() {
                Some(result) => result,
                None => Ok(None),
            }
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
        ) -> Result<fxrate_service::ports::FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError>
        {
            Ok(fxrate_service::ports::FxRateSnapshotInsertOutcome::Duplicate)
        }
    }

    #[derive(Clone)]
    struct FakeEmbeddingGenerator {
        state: SharedState,
    }

    #[async_trait::async_trait]
    impl EmbeddingGenerator for FakeEmbeddingGenerator {
        async fn embed_product(
            &self,
            _: &EmbeddingText,
            _: Option<&embedding::EmbeddingText>,
            _: Option<&embedding::EmbeddingImageUrl>,
        ) -> Result<EmbeddingVector, EmbeddingError> {
            Err(EmbeddingError::InvalidInput {
                reason: "test generator supports queries only",
            })
        }

        async fn embed_search_query(
            &self,
            query: &EmbeddingText,
        ) -> Result<EmbeddingVector, EmbeddingError> {
            let mut state = lock_state(&self.state);
            state.embedding_queries.push(query.as_str().to_owned());
            match state.embedding_result.take() {
                Some(result) => result,
                None => EmbeddingVector::try_new(vec![1.0; embedding::EMBEDDING_DIMENSIONS]),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductListingUserStateReader for FakeUserStatesReader {
        async fn find_for_user(
            &self,
            lookup: &ProductListingUserStateLookup,
        ) -> Result<
            HashMap<ProductListingId, ProductListingUserState>,
            ProductListingUserStateReadError,
        > {
            let mut state = lock_state(&self.state);
            state.user_state_lookups.push(lookup.clone());
            match state.user_states_result.take() {
                Some(result) => result,
                None => Ok(HashMap::new()),
            }
        }
    }

    fn handler(
        state: &SharedState,
    ) -> SearchProductListingsHandler<
        FakeUnitOfWork,
        FakeSearchReader,
        FakeFxRateSnapshotRepositoryFactory,
        FakeEmbeddingGenerator,
        FakeUserStatesReader,
    > {
        SearchProductListingsHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(state),
            },
            search_reader(state),
            FakeFxRateSnapshotRepositoryFactory {
                state: Arc::clone(state),
            },
            FakeEmbeddingGenerator {
                state: Arc::clone(state),
            },
            FakeUserStatesReader {
                state: Arc::clone(state),
            },
        )
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn user_context(user_id: UserId) -> OperationContext {
        OperationContext {
            principal: Principal::User(user_id),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn search_result() -> Result<ProductListingSearchReadResult, url::ParseError> {
        Ok(ProductListingSearchReadResult {
            items: vec![ProductListingSummary {
                product_id: ProductListingId::new(),
                product_slug_id: ProductListingSlugId::from("cabinet-abcdef"),
                event_id: EventId::new(),
                shop_id: ShopId::new(),
                seller_id: ShopId::new(),
                shop_listing_id: ShopListingId::new(),
                shop_name: ShopName::from("Shop"),
                shop_slug_id: ShopSlugId::from("shop"),
                title: Some(Localized {
                    localization: Language::En,
                    payload: Title::from("Cabinet"),
                }),
                display_price: Some(Price::new(MonetaryAmount::from(100_u64), Currency::Eur)),
                price_valuation: ProductListingSummaryPriceValuation::Current {
                    fx_rate_id: FxRateId::new(),
                    captured_at: OffsetDateTime::UNIX_EPOCH,
                },
                state: ProductState::Listed,
                lifecycle: ProductLifecycle::Active,
                url: Url::parse("https://shop.example/products/1")?,
                view_url: Url::parse("https://aura.example/products/cabinet-abcdef")?,
                images: IndexSet::<ProductListingImage>::new(),
                updated: OffsetDateTime::UNIX_EPOCH,
            }],
            cursor: Cursor {
                size: 21,
                search_after: Some(Value::String("next".to_owned())),
            },
            total: Some(1),
        })
    }

    fn snapshot() -> Result<FxRateSnapshot, fxrate_core::FxRateSnapshotError> {
        NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| FxRateQuote::new(currency, FX_RATE_SCALE)),
        )
        .and_then(|snapshot| Ok(snapshot.into_persisted(1_i64.try_into()?)))
    }

    fn request() -> SearchProductListingsRequest {
        SearchProductListingsRequest {
            search: ProductListingSearch::new(Language::En, Currency::Eur),
            sort: None,
            cursor: None,
        }
    }

    fn request_with_text_query() -> Result<SearchProductListingsRequest, Box<dyn std::error::Error>>
    {
        Ok(SearchProductListingsRequest {
            search: ProductListingSearch::new(Language::En, Currency::Eur)
                .with_product_listing_query("vintage brass lamp".try_into()?),
            sort: None,
            cursor: None,
        })
    }

    #[tokio::test]
    async fn should_search_products_when_reader_succeeds() -> Result<(), Box<dyn std::error::Error>>
    {
        let state = state();
        let expected = search_result()?;
        lock_state(&state).search_result = Some(Ok(expected.clone()));

        let result = handler(&state).execute(&context(), request()).await?;

        assert_eq!(expected.items[0], result.items[0].item);
        assert_eq!(None, result.items[0].user_state);
        assert!(matches!(
            result.cursor.search_after,
            Some(ProductListingSearchCursor { search_after: Value::String(value), .. }) if value == "next"
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_use_hybrid_search_when_query_embedding_succeeds()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let expected = search_result()?;
        lock_state(&state).hybrid_search_result = Some(Ok(expected.clone()));

        let result = handler(&state)
            .execute(&context(), request_with_text_query()?)
            .await?;

        assert_eq!(expected.items[0], result.items[0].item);
        assert_eq!(None, result.items[0].user_state);
        assert!(matches!(
            result.cursor.search_after,
            Some(ProductListingSearchCursor { search_after: Value::String(value), .. }) if value == "next"
        ));
        let state = lock_state(&state);
        assert!(state.used_hybrid_search);
        assert!(matches!(
            state.embedding_queries.as_slice(),
            [query] if query == "vintage brass lamp"
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_fall_back_to_bm25_when_query_embedding_fails()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let expected = search_result()?;
        lock_state(&state).embedding_result = Some(Err(EmbeddingError::InvalidInput {
            reason: "embedding unavailable",
        }));
        lock_state(&state).search_result = Some(Ok(expected.clone()));

        let result = handler(&state)
            .execute(&context(), request_with_text_query()?)
            .await?;

        assert_eq!(expected.items[0], result.items[0].item);
        assert_eq!(None, result.items[0].user_state);
        assert!(matches!(
            result.cursor.search_after,
            Some(ProductListingSearchCursor { search_after: Value::String(value), .. }) if value == "next"
        ));
        let state = lock_state(&state);
        assert!(!state.used_hybrid_search);
        assert_eq!(1, state.embedding_queries.len());
        Ok(())
    }

    #[tokio::test]
    async fn should_keep_cursor_fx_snapshot_for_the_next_page()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let snapshot = snapshot()?;
        lock_state(&state).fx_rate_snapshot =
            Some(Err(FxRateSnapshotRepositoryError::ReadFailed {
                source: box_error(std::io::Error::other("latest snapshot must not be read")),
            }));
        lock_state(&state).fx_rate_snapshot_by_id = Some(Ok(Some(snapshot.clone())));
        lock_state(&state).search_result = Some(Ok(search_result()?));
        let mut request = request();
        request.cursor = Some(Cursor {
            size: 21,
            search_after: Some(ProductListingSearchCursor {
                fx_rate_id: snapshot.id(),
                search_after: Value::Array(vec![Value::String("previous".to_owned())]),
            }),
        });

        let result = handler(&state).execute(&context(), request).await?;

        assert!(matches!(
            result.cursor.search_after,
            Some(ProductListingSearchCursor { fx_rate_id, search_after: Value::String(value) })
                if fx_rate_id == snapshot.id() && value == "next"
        ));
        let state = lock_state(&state);
        assert!(matches!(
            state.read_requests.as_slice(),
            [request] if request.cursor.as_ref().and_then(|cursor| cursor.search_after.as_ref())
                == Some(&Value::Array(vec![Value::String("previous".to_owned())]))
                && request.compiled_search.price_filter_plan.fx_rate_id == snapshot.id()
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_pass_one_compiled_request_with_a_pinned_price_filter_plan_to_the_reader()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let snapshot = snapshot()?;
        lock_state(&state).fx_rate_snapshot = Some(Ok(Some(snapshot.clone())));
        lock_state(&state).search_result = Some(Ok(search_result()?));
        let mut request = request();
        request.search.price_query = Some(domain_primitives::query::range_query::RangeQuery {
            min: Some(100_u64.into()),
            max: Some(200_u64.into()),
        });

        handler(&state).execute(&context(), request).await?;

        let state = lock_state(&state);
        assert!(matches!(
            state.read_requests.as_slice(),
            [request] if request.compiled_search.price_filter_plan.fx_rate_id == snapshot.id()
                && request.compiled_search.price_filter_plan.target_currency == Currency::Eur
                && request.compiled_search.price_filter_plan.sold_display_range.min == Some(100_u64.into())
                && request.compiled_search.price_filter_plan.sold_display_range.max == Some(200_u64.into())
                && request.compiled_search.search.price_query.is_some()
        ));
        assert_eq!(vec![1], state.reader_observed_commit_counts);
        Ok(())
    }

    #[tokio::test]
    async fn should_fail_when_latest_fx_rate_snapshot_is_missing() {
        let state = state();
        lock_state(&state).fx_rate_snapshot = Some(Ok(None));

        let result = handler(&state).execute(&context(), request()).await;

        assert!(matches!(
            result,
            Err(SearchProductListingsError::FxRateSnapshotMissing)
        ));
        assert_eq!(1, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_fail_when_latest_fx_rate_snapshot_transaction_cannot_begin() {
        let state = state();
        lock_state(&state).begin_error = true;

        let result = handler(&state).execute(&context(), request()).await;

        assert!(matches!(
            result,
            Err(SearchProductListingsError::BeginFxRateSnapshotTransactionFailed { .. })
        ));
        assert_eq!(0, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_fail_when_latest_fx_rate_snapshot_transaction_cannot_commit() {
        let state = state();
        lock_state(&state).commit_error = true;

        let result = handler(&state).execute(&context(), request()).await;

        assert!(matches!(
            result,
            Err(SearchProductListingsError::CommitFxRateSnapshotTransactionFailed { .. })
        ));
        assert_eq!(1, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_fail_when_latest_fx_rate_snapshot_is_invalid() {
        let state = state();
        lock_state(&state).fx_rate_snapshot = Some(Err(
            FxRateSnapshotRepositoryError::InvalidPersistedSnapshot {
                source: box_error(std::io::Error::other("invalid persisted snapshot")),
            },
        ));

        let result = handler(&state).execute(&context(), request()).await;

        assert!(matches!(
            result,
            Err(SearchProductListingsError::FxRateSnapshotInvalid { .. })
        ));
    }

    #[tokio::test]
    async fn should_fail_when_latest_fx_rate_snapshot_read_fails() {
        let state = state();
        lock_state(&state).fx_rate_snapshot =
            Some(Err(FxRateSnapshotRepositoryError::ReadFailed {
                source: box_error(std::io::Error::other("postgres unavailable")),
            }));

        let result = handler(&state).execute(&context(), request()).await;

        assert!(matches!(
            result,
            Err(SearchProductListingsError::FxRateSnapshotReadFailed { .. })
        ));
        assert_eq!(0, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_hydrate_search_results_once_for_authenticated_user()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let user_id = UserId::new();
        let expected = search_result()?;
        let product_id = expected.items[0].product_id;
        let notification_id = notification_core::notification_id::NotificationId::new();
        let mut user_state = ProductListingUserState::default();
        user_state.watchlist.watching = true;
        user_state.watchlist.notifications = true;
        user_state.notification.unseen_notification_ids = vec![notification_id];
        lock_state(&state).search_result = Some(Ok(expected));
        lock_state(&state).user_states_result = Some(Ok(HashMap::from([(product_id, user_state)])));

        let result = handler(&state)
            .execute(&user_context(user_id), request())
            .await?;

        let state = lock_state(&state);
        assert_eq!(1, state.user_state_lookups.len());
        assert_eq!(user_id, state.user_state_lookups[0].user_id);
        assert_eq!(1, state.user_state_lookups[0].product_ids.len());
        assert_eq!(product_id, state.user_state_lookups[0].product_ids[0]);
        let user_state = result.items[0]
            .user_state
            .as_ref()
            .ok_or("missing user state")?;
        assert!(user_state.watchlist.watching);
        assert_eq!(
            vec![notification_id],
            user_state.notification.unseen_notification_ids
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_fail_when_authenticated_product_user_state_read_fails() {
        let state = state();
        let user_id = UserId::new();
        let expected = match search_result() {
            Ok(result) => result,
            Err(error) => panic!("failed to build product search result: {error}"),
        };
        lock_state(&state).search_result = Some(Ok(expected));
        lock_state(&state).user_states_result =
            Some(Err(ProductListingUserStateReadError::QueryFailed {
                source: box_error(std::io::Error::other("postgres unavailable")),
            }));

        let result = handler(&state)
            .execute(&user_context(user_id), request())
            .await;

        assert!(matches!(
            result,
            Err(SearchProductListingsError::ProductListingUserStateQueryFailed { .. })
        ));
    }

    #[tokio::test]
    async fn should_redact_hidden_search_result_for_authenticated_user()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let user_id = UserId::new();
        let expected = search_result()?;
        let product_id = expected.items[0].product_id;
        let mut user_state = ProductListingUserState::default();
        user_state.search_filter.hidden = true;
        lock_state(&state).search_result = Some(Ok(expected));
        lock_state(&state).user_states_result = Some(Ok(HashMap::from([(product_id, user_state)])));

        let result = handler(&state)
            .execute(&user_context(user_id), request())
            .await?;

        assert_eq!(
            ProductListingId::from(uuid::Uuid::nil()),
            result.items[0].item.product_id
        );
        assert_eq!(
            Some(true),
            result.items[0]
                .user_state
                .as_ref()
                .map(|state| state.search_filter.hidden)
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_map_reader_error_when_search_products_read_fails() {
        let state = state();
        lock_state(&state).search_result = Some(Err(
            ProductListingSearchReadError::ProductListingSearchQueryFailed,
        ));

        let result = handler(&state).execute(&context(), request()).await;

        assert!(matches!(
            result,
            Err(SearchProductListingsError::ProductListingSearchQueryFailed)
        ));
    }

    #[test]
    fn should_map_all_search_products_read_errors() {
        assert!(matches!(
            SearchProductListingsError::from(
                ProductListingSearchReadError::ProductListingSearchQueryFailed
            ),
            SearchProductListingsError::ProductListingSearchQueryFailed
        ));
        assert!(matches!(
            SearchProductListingsError::from(
                ProductListingSearchReadError::ProductListingSearchReadModelInvalid
            ),
            SearchProductListingsError::ProductListingSearchReadModelInvalid
        ));
    }
}
