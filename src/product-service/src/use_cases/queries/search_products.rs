use crate::ports::{
    CompiledProductSearch, ProductPriceFilterPlan, ProductSearchReadError,
    ProductSearchReadRequest, ProductSearchReader, ProductUserStateReader,
};
use crate::use_cases::queries::product_summary_personalization::{
    ProductSummaryPersonalizationError, hydrate_product_summaries,
};
use application::transaction::{Transaction, UnitOfWork};
use common::error::boxed::{BoxError, box_error};
use common::event_id::EventId;
use common::fx_rate_id::FxRateId;
use common::operation_context::{OperationContext, Principal};
use common::pagination::cursor::{Cursor, CursoredResult};
use common::personalized::Personalized;
use common::product_id::ProductId;
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_slug_id::ProductSlugId;
use common::product_state::domain::ProductState;
use common::shops_product_id::ShopsProductId;
use common::sort::Sort;
use embedding::{EmbeddingGenerator, EmbeddingText};
use fxrate_core::{FxRateSnapshot, FxRateSnapshotError};
use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use localization::Language;
use localization::Localized;
use money::Price;
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use shop_core::shop_slug_id::ShopSlugId;

use indexmap::IndexSet;
use notification_service::ports::all_notifications_reader::AllNotificationsReader;
use product_core::product_image::ProductImage;
use product_core::product_search::ProductSearch;
use product_core::sort_product_field::SortProductField;
use product_core::title::Title;
use product_core::user_state::ProductUserState;
use serde_json::Value;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchProductsRequest {
    pub search: ProductSearch,
    pub sort: Option<Sort<SortProductField>>,
    pub cursor: Option<Cursor<ProductSearchCursor>>,
}

/// Opaque Product-owned continuation state.
///
/// The OpenSearch sort token is scoped to one immutable persisted FX snapshot, so active
/// Product presentation and price-range membership cannot change within a cursor chain.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductSearchCursor {
    pub fx_rate_id: FxRateId,
    pub search_after: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductSummary {
    pub product_id: ProductId,
    pub product_slug_id: ProductSlugId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub shop_slug_id: ShopSlugId,
    pub title: Option<Localized<Language, Title>>,
    pub display_price: Option<Price>,
    pub price_valuation: ProductSummaryPriceValuation,
    pub state: ProductState,
    pub lifecycle: ProductLifecycle,
    pub url: Url,
    pub view_url: Url,
    pub images: IndexSet<ProductImage>,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductSummaryPriceValuation {
    Current {
        fx_rate_id: FxRateId,
        captured_at: OffsetDateTime,
    },
    Sale {
        fx_rate_id: FxRateId,
        sold_at: OffsetDateTime,
    },
}

pub type PersonalizedProductSummary = Personalized<ProductSummary, ProductUserState>;
pub type ProductSearchReadResult = CursoredResult<ProductSummary, Value>;
pub type SearchProductsResult = CursoredResult<PersonalizedProductSummary, ProductSearchCursor>;

#[derive(Debug, thiserror::Error)]
pub enum SearchProductsError {
    #[error("product search query failed")]
    ProductSearchQueryFailed,
    #[error("product search read model is invalid")]
    ProductSearchReadModelInvalid,
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
    ProductUserStateQueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("product user state read model is invalid")]
    ProductUserStateReadModelInvalid {
        #[source]
        source: BoxError,
    },
    #[error("product notification read failed")]
    ProductNotificationReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("product user state is missing")]
    ProductUserStateMissing,
    #[error("hidden product summary could not be constructed")]
    HiddenProductSummaryInvalid {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait SearchProductsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchProductsRequest,
    ) -> Result<SearchProductsResult, SearchProductsError>;
}

pub struct SearchProductsHandler<UoW, R, F, E, U, N> {
    unit_of_work: UoW,
    reader: R,
    fx_rates: F,
    embeddings: E,
    user_states: U,
    notifications: N,
}

impl<UoW, R, F, E, U, N> SearchProductsHandler<UoW, R, F, E, U, N> {
    pub fn new(
        unit_of_work: UoW,
        reader: R,
        fx_rates: F,
        embeddings: E,
        user_states: U,
        notifications: N,
    ) -> Self {
        Self {
            unit_of_work,
            reader,
            fx_rates,
            embeddings,
            user_states,
            notifications,
        }
    }
}

#[async_trait::async_trait]
impl<UoW, R, F, E, U, N> SearchProductsUseCase for SearchProductsHandler<UoW, R, F, E, U, N>
where
    UoW: UnitOfWork,
    R: ProductSearchReader,
    F: FxRateSnapshotRepositoryFactory<UoW::Tx>,
    E: EmbeddingGenerator,
    U: ProductUserStateReader,
    N: AllNotificationsReader,
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
        request: SearchProductsRequest,
    ) -> Result<SearchProductsResult, SearchProductsError> {
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
        let read_request = ProductSearchReadRequest {
            compiled_search: CompiledProductSearch {
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
                search_after: result
                    .cursor
                    .search_after
                    .map(|search_after| ProductSearchCursor {
                        fx_rate_id,
                        search_after,
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
            hydrate_product_summaries(
                &mut result.items,
                user_id,
                &self.user_states,
                &self.notifications,
            )
            .await?;
        }
        Ok(result)
    }
}

async fn load_fx_rate_snapshot<UoW, F>(
    unit_of_work: &UoW,
    fx_rates: &F,
    pinned_fx_rate_id: Option<FxRateId>,
    valuation_at: OffsetDateTime,
) -> Result<FxRateSnapshot, SearchProductsError>
where
    UoW: UnitOfWork,
    F: FxRateSnapshotRepositoryFactory<UoW::Tx>,
{
    let mut tx = unit_of_work.begin().await.map_err(|source| {
        SearchProductsError::BeginFxRateSnapshotTransactionFailed {
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
        SearchProductsError::CommitFxRateSnapshotTransactionFailed {
            source: box_error(source),
        }
    })?;
    snapshot.ok_or(SearchProductsError::FxRateSnapshotMissing)
}

fn compile_price_filter(
    snapshot: FxRateSnapshot,
    request: &SearchProductsRequest,
) -> Result<ProductPriceFilterPlan, SearchProductsError> {
    ProductPriceFilterPlan::compile(
        snapshot,
        request.search.currency,
        request.search.price_query,
    )
    .map_err(
        |error: FxRateSnapshotError| SearchProductsError::FxRateSnapshotInvalid {
            source: box_error(error),
        },
    )
}

fn fx_rate_snapshot_read_error(error: FxRateSnapshotRepositoryError) -> SearchProductsError {
    match error {
        FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { source } => {
            SearchProductsError::FxRateSnapshotInvalid { source }
        }
        FxRateSnapshotRepositoryError::InsertFailed { source }
        | FxRateSnapshotRepositoryError::ReadFailed { source } => {
            SearchProductsError::FxRateSnapshotReadFailed { source }
        }
        FxRateSnapshotRepositoryError::CapturedAtNotMonotonic => {
            SearchProductsError::FxRateSnapshotMissing
        }
    }
}

fn hybrid_embedding_query(request: &SearchProductsRequest) -> Option<EmbeddingText> {
    if !matches!(
        request.sort.as_ref().map(|sort| sort.sort),
        None | Some(SortProductField::Score)
    ) {
        return None;
    }

    let text = request
        .search
        .product_query
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

fn personalization_user_id(principal: &Principal) -> Option<common::user_id::UserId> {
    match principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}

impl From<ProductSearchReadError> for SearchProductsError {
    fn from(error: ProductSearchReadError) -> Self {
        match error {
            ProductSearchReadError::ProductSearchQueryFailed => Self::ProductSearchQueryFailed,
            ProductSearchReadError::ProductSearchReadModelInvalid => {
                Self::ProductSearchReadModelInvalid
            }
        }
    }
}

impl From<ProductSummaryPersonalizationError> for SearchProductsError {
    fn from(error: ProductSummaryPersonalizationError) -> Self {
        match error {
            ProductSummaryPersonalizationError::UserStateQueryFailed { source } => {
                Self::ProductUserStateQueryFailed { source }
            }
            ProductSummaryPersonalizationError::UserStateReadModelInvalid { source } => {
                Self::ProductUserStateReadModelInvalid { source }
            }
            ProductSummaryPersonalizationError::NotificationReadFailed { source } => {
                Self::ProductNotificationReadFailed { source }
            }
            ProductSummaryPersonalizationError::UserStateMissing { .. } => {
                Self::ProductUserStateMissing
            }
            ProductSummaryPersonalizationError::HiddenProductSummaryInvalid { source } => {
                Self::HiddenProductSummaryInvalid { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{ProductUserStateLookup, ProductUserStateReadError};
    use application::transaction::{TransactionError, UnitOfWork};
    use common::error::boxed::box_error;
    use common::event_id::EventId;
    use common::fx_rate_id::FxRateId;
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use common::user_id::UserId;
    use embedding::{EmbeddingError, EmbeddingVector};
    use fxrate_core::{
        FX_RATE_SCALE, FxRateQuote, FxRateSnapshot, FxRateSource, NewFxRateSnapshot,
    };
    use fxrate_service::ports::FxRateSnapshotRepositoryFactory;
    use localization::Language;
    use money::Currency;
    use money::MonetaryAmount;

    use notification_core::notification::{NotificationPayload, NotificationWatchlistPayload};
    use notification_core::notification_id::NotificationId;
    use notification_service::ports::all_notifications_reader::{
        AllNotificationsReadError, AllNotificationsReadItem, AllNotificationsReader,
    };
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, MutexGuard};
    use strum::IntoEnumIterator;

    #[derive(Debug, Default)]
    struct FakeState {
        search_result: Option<Result<ProductSearchReadResult, ProductSearchReadError>>,
        hybrid_search_result: Option<Result<ProductSearchReadResult, ProductSearchReadError>>,
        read_requests: Vec<ProductSearchReadRequest>,
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
        user_states_result:
            Option<Result<HashMap<ProductId, ProductUserState>, ProductUserStateReadError>>,
        notifications_result:
            Option<Result<Vec<AllNotificationsReadItem>, AllNotificationsReadError>>,
        user_state_lookups: Vec<ProductUserStateLookup>,
        notification_requests: Vec<UserId>,
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

    #[derive(Clone)]
    struct FakeNotificationsReader {
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
    impl ProductSearchReader for FakeSearchReader {
        async fn search(
            &self,
            request: &ProductSearchReadRequest,
        ) -> Result<ProductSearchReadResult, ProductSearchReadError> {
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
            request: &ProductSearchReadRequest,
            _embedding: &[f32],
        ) -> Result<ProductSearchReadResult, ProductSearchReadError> {
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
    impl ProductUserStateReader for FakeUserStatesReader {
        async fn find_for_user(
            &self,
            lookup: &ProductUserStateLookup,
        ) -> Result<HashMap<ProductId, ProductUserState>, ProductUserStateReadError> {
            let mut state = lock_state(&self.state);
            state.user_state_lookups.push(lookup.clone());
            match state.user_states_result.take() {
                Some(result) => result,
                None => Ok(HashMap::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl AllNotificationsReader for FakeNotificationsReader {
        async fn list_all_by_user(
            &self,
            user_id: &UserId,
        ) -> Result<Vec<AllNotificationsReadItem>, AllNotificationsReadError> {
            let mut state = lock_state(&self.state);
            state.notification_requests.push(*user_id);
            match state.notifications_result.take() {
                Some(result) => result,
                None => Ok(Vec::new()),
            }
        }
    }

    fn handler(
        state: &SharedState,
    ) -> SearchProductsHandler<
        FakeUnitOfWork,
        FakeSearchReader,
        FakeFxRateSnapshotRepositoryFactory,
        FakeEmbeddingGenerator,
        FakeUserStatesReader,
        FakeNotificationsReader,
    > {
        SearchProductsHandler::new(
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
            FakeNotificationsReader {
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

    fn notification(
        user_id: UserId,
        product_id: ProductId,
        event_id: EventId,
        seen: bool,
    ) -> Result<AllNotificationsReadItem, url::ParseError> {
        let url = Url::parse("https://example.test/product")?;
        Ok(AllNotificationsReadItem {
            user_id,
            origin_event_id: event_id,
            notification_id: NotificationId::new(),
            notification_type: None,
            notification_payload: NotificationPayload::Watchlist {
                product_id,
                shop_id: ShopId::new(),
                shops_product_id: ShopsProductId::from("product"),
                shop_slug_id: ShopSlugId::from("shop"),
                product_slug_id: ProductSlugId::from("product"),
                shop_name: ShopName::from("Shop"),
                title: None,
                image: None,
                url: url.clone(),
                view_url: url,
                watchlist_payload: NotificationWatchlistPayload::StateChange {
                    old_state: ProductState::Listed,
                    new_state: ProductState::Available,
                },
            },
            seen,
            external: false,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        })
    }

    fn search_result() -> Result<ProductSearchReadResult, url::ParseError> {
        Ok(ProductSearchReadResult {
            items: vec![ProductSummary {
                product_id: ProductId::new(),
                product_slug_id: ProductSlugId::from("cabinet-abcdef"),
                event_id: EventId::new(),
                shop_id: ShopId::new(),
                seller_id: ShopId::new(),
                shops_product_id: ShopsProductId::new(),
                shop_name: ShopName::from("Shop"),
                shop_slug_id: ShopSlugId::from("shop"),
                title: Some(Localized {
                    localization: Language::En,
                    payload: Title::from("Cabinet"),
                }),
                display_price: Some(Price::new(MonetaryAmount::from(100_u64), Currency::Eur)),
                price_valuation: ProductSummaryPriceValuation::Current {
                    fx_rate_id: FxRateId::new(),
                    captured_at: OffsetDateTime::UNIX_EPOCH,
                },
                state: ProductState::Listed,
                lifecycle: ProductLifecycle::Active,
                url: Url::parse("https://shop.example/products/1")?,
                view_url: Url::parse("https://aura.example/products/cabinet-abcdef")?,
                images: IndexSet::<ProductImage>::new(),
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

    fn request() -> SearchProductsRequest {
        SearchProductsRequest {
            search: ProductSearch::new(Language::En, Currency::Eur),
            sort: None,
            cursor: None,
        }
    }

    fn request_with_text_query() -> Result<SearchProductsRequest, Box<dyn std::error::Error>> {
        Ok(SearchProductsRequest {
            search: ProductSearch::new(Language::En, Currency::Eur)
                .with_product_query("vintage brass lamp".try_into()?),
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
            Some(ProductSearchCursor { search_after: Value::String(value), .. }) if value == "next"
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
            Some(ProductSearchCursor { search_after: Value::String(value), .. }) if value == "next"
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
            Some(ProductSearchCursor { search_after: Value::String(value), .. }) if value == "next"
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
            search_after: Some(ProductSearchCursor {
                fx_rate_id: snapshot.id(),
                search_after: Value::Array(vec![Value::String("previous".to_owned())]),
            }),
        });

        let result = handler(&state).execute(&context(), request).await?;

        assert!(matches!(
            result.cursor.search_after,
            Some(ProductSearchCursor { fx_rate_id, search_after: Value::String(value) })
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
        request.search.price_query = Some(common::query::range_query::RangeQuery {
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
            Err(SearchProductsError::FxRateSnapshotMissing)
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
            Err(SearchProductsError::BeginFxRateSnapshotTransactionFailed { .. })
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
            Err(SearchProductsError::CommitFxRateSnapshotTransactionFailed { .. })
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
            Err(SearchProductsError::FxRateSnapshotInvalid { .. })
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
            Err(SearchProductsError::FxRateSnapshotReadFailed { .. })
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
        let event_id = EventId::new();
        let mut user_state = ProductUserState::default();
        user_state.watchlist.watching = true;
        user_state.watchlist.notifications = true;
        lock_state(&state).search_result = Some(Ok(expected));
        lock_state(&state).user_states_result = Some(Ok(HashMap::from([(product_id, user_state)])));
        lock_state(&state).notifications_result = Some(Ok(vec![notification(
            user_id, product_id, event_id, false,
        )?]));

        let result = handler(&state)
            .execute(&user_context(user_id), request())
            .await?;

        assert_eq!(
            Some(user_id),
            lock_state(&state).notification_requests.first().copied()
        );
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
        assert!(!user_state.notification.seen);
        assert_eq!(Some(event_id), user_state.notification.origin_event_id);
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
        lock_state(&state).user_states_result = Some(Err(ProductUserStateReadError::QueryFailed {
            source: box_error(std::io::Error::other("postgres unavailable")),
        }));

        let result = handler(&state)
            .execute(&user_context(user_id), request())
            .await;

        assert!(matches!(
            result,
            Err(SearchProductsError::ProductUserStateQueryFailed { .. })
        ));
    }

    #[tokio::test]
    async fn should_fail_when_authenticated_notification_read_fails() {
        let state = state();
        let user_id = UserId::new();
        let expected = match search_result() {
            Ok(result) => result,
            Err(error) => panic!("failed to build product search result: {error}"),
        };
        let product_id = expected.items[0].product_id;
        lock_state(&state).search_result = Some(Ok(expected));
        lock_state(&state).user_states_result = Some(Ok(HashMap::from([(
            product_id,
            ProductUserState::default(),
        )])));
        lock_state(&state).notifications_result =
            Some(Err(AllNotificationsReadError::OperationFailed {
                source: box_error(std::io::Error::other("dynamodb unavailable")),
            }));

        let result = handler(&state)
            .execute(&user_context(user_id), request())
            .await;

        assert!(matches!(
            result,
            Err(SearchProductsError::ProductNotificationReadFailed { .. })
        ));
    }

    #[tokio::test]
    async fn should_redact_hidden_search_result_for_authenticated_user()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let user_id = UserId::new();
        let expected = search_result()?;
        let product_id = expected.items[0].product_id;
        let mut user_state = ProductUserState::default();
        user_state.search_filter.hidden = true;
        lock_state(&state).search_result = Some(Ok(expected));
        lock_state(&state).user_states_result = Some(Ok(HashMap::from([(product_id, user_state)])));

        let result = handler(&state)
            .execute(&user_context(user_id), request())
            .await?;

        assert_eq!(
            ProductId::from(uuid::Uuid::nil()),
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
        lock_state(&state).search_result =
            Some(Err(ProductSearchReadError::ProductSearchQueryFailed));

        let result = handler(&state).execute(&context(), request()).await;

        assert!(matches!(
            result,
            Err(SearchProductsError::ProductSearchQueryFailed)
        ));
    }

    #[test]
    fn should_map_all_search_products_read_errors() {
        assert!(matches!(
            SearchProductsError::from(ProductSearchReadError::ProductSearchQueryFailed),
            SearchProductsError::ProductSearchQueryFailed
        ));
        assert!(matches!(
            SearchProductsError::from(ProductSearchReadError::ProductSearchReadModelInvalid),
            SearchProductsError::ProductSearchReadModelInvalid
        ));
    }
}
