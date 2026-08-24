use crate::ports::{
    ProductEmbeddingLookup, ProductEmbeddingReadError, ProductEmbeddingReader,
    ProductEmbeddingReaderFactory, ProductPriceFilterPlan, ProductSimilarProductsReadError,
    ProductSimilarProductsReader, ProductSimilarProductsRequest, ProductUserStateReader,
};
use crate::use_cases::PersonalizedProductSummary;
use crate::use_cases::queries::product_summary_personalization::{
    ProductSummaryPersonalizationError, hydrate_product_summaries,
};
use application::error::{BoxError, box_error};
use application::operation_context::{OperationContext, Principal};
use application::personalized::Personalized;
use application::transaction::{Transaction, UnitOfWork};
use localization::Language;
use money::Currency;

use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};

use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct GetSimilarProductsRequest {
    pub lookup: ProductEmbeddingLookup,
    pub language: Language,
    pub currency: Currency,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GetSimilarProductsResult {
    EmbeddingPending,
    Ready(Vec<PersonalizedProductSummary>),
}

#[derive(Debug, thiserror::Error)]
pub enum GetSimilarProductsError {
    #[error("product not found")]
    NotFound,
    #[error("product embedding query failed")]
    ProductEmbeddingQueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("similarity search is unavailable")]
    SimilaritySearchUnavailable,
    #[error("failed to begin get similar products transaction")]
    BeginTransactionFailed,
    #[error("failed to commit get similar products transaction")]
    CommitTransactionFailed,
    #[error("no persisted FX snapshot is available for similar product pricing")]
    PricingFxSnapshotMissing,
    #[error("FX snapshot lookup is temporarily unavailable for similar product pricing")]
    PricingFxSnapshotUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("persisted FX snapshot is invalid for similar product pricing")]
    PricingFxSnapshotInvalid {
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

    #[error("product user state is missing")]
    ProductUserStateMissing,
    #[error("hidden product summary could not be constructed")]
    HiddenProductSummaryInvalid {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait GetSimilarProductsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetSimilarProductsRequest,
    ) -> Result<GetSimilarProductsResult, GetSimilarProductsError>;
}

pub struct GetSimilarProductsHandler<U, E, F, S, P> {
    unit_of_work: U,
    embedding_reader: E,
    fx_rates: F,
    similar_products_reader: S,
    user_states: P,
}

impl<U, E, F, S, P> GetSimilarProductsHandler<U, E, F, S, P> {
    pub fn new(
        unit_of_work: U,
        embedding_reader: E,
        fx_rates: F,
        similar_products_reader: S,
        user_states: P,
    ) -> Self {
        Self {
            unit_of_work,
            embedding_reader,
            fx_rates,
            similar_products_reader,
            user_states,
        }
    }
}

#[async_trait::async_trait]
impl<U, E, F, S, P> GetSimilarProductsUseCase for GetSimilarProductsHandler<U, E, F, S, P>
where
    U: UnitOfWork,
    E: ProductEmbeddingReaderFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
    S: ProductSimilarProductsReader,
    P: ProductUserStateReader,
{
    #[tracing::instrument(
        name = "get_similar_products",
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
        request: GetSimilarProductsRequest,
    ) -> Result<GetSimilarProductsResult, GetSimilarProductsError> {
        let valuation_at = OffsetDateTime::now_utc();
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetSimilarProductsError::BeginTransactionFailed)?;
        let seed = self
            .embedding_reader
            .in_transaction(&mut tx)
            .find_embedding(&request.lookup)
            .await?
            .ok_or(GetSimilarProductsError::NotFound)?;

        let Some(embedding) = seed.embedding else {
            tx.commit()
                .await
                .map_err(|_| GetSimilarProductsError::CommitTransactionFailed)?;

            return Ok(GetSimilarProductsResult::EmbeddingPending);
        };

        let snapshot = self
            .fx_rates
            .in_transaction(&mut tx)
            .find_latest_at_or_before(valuation_at)
            .await?
            .ok_or(GetSimilarProductsError::PricingFxSnapshotMissing)?;
        let price_filter_plan = ProductPriceFilterPlan::compile(snapshot, request.currency, None)
            .map_err(|source| {
            GetSimilarProductsError::PricingFxSnapshotInvalid {
                source: box_error(source),
            }
        })?;

        tx.commit()
            .await
            .map_err(|_| GetSimilarProductsError::CommitTransactionFailed)?;

        let products = self
            .similar_products_reader
            .find_similar_products(&ProductSimilarProductsRequest::new(
                seed.product_id,
                embedding,
                request.language,
                price_filter_plan,
            ))
            .await?;
        let mut products = products
            .into_iter()
            .map(|item| Personalized {
                item,
                user_state: None,
            })
            .collect::<Vec<_>>();
        if let Some(user_id) = personalization_user_id(&context.principal) {
            hydrate_product_summaries(&mut products, user_id, &self.user_states).await?;
        }

        Ok(GetSimilarProductsResult::Ready(products))
    }
}

fn personalization_user_id(principal: &Principal) -> Option<user_core::user_id::UserId> {
    match principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}

impl From<ProductEmbeddingReadError> for GetSimilarProductsError {
    fn from(error: ProductEmbeddingReadError) -> Self {
        match error {
            ProductEmbeddingReadError::ProductEmbeddingQueryFailed { source } => {
                Self::ProductEmbeddingQueryFailed { source }
            }
        }
    }
}

impl From<ProductSimilarProductsReadError> for GetSimilarProductsError {
    fn from(_: ProductSimilarProductsReadError) -> Self {
        Self::SimilaritySearchUnavailable
    }
}

impl From<FxRateSnapshotRepositoryError> for GetSimilarProductsError {
    fn from(error: FxRateSnapshotRepositoryError) -> Self {
        match error {
            FxRateSnapshotRepositoryError::InsertFailed { source }
            | FxRateSnapshotRepositoryError::ReadFailed { source } => {
                Self::PricingFxSnapshotUnavailable { source }
            }
            FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { source } => {
                Self::PricingFxSnapshotInvalid { source }
            }
            FxRateSnapshotRepositoryError::CapturedAtNotMonotonic => Self::PricingFxSnapshotMissing,
        }
    }
}

impl From<ProductSummaryPersonalizationError> for GetSimilarProductsError {
    fn from(error: ProductSummaryPersonalizationError) -> Self {
        match error {
            ProductSummaryPersonalizationError::UserStateQueryFailed { source } => {
                Self::ProductUserStateQueryFailed { source }
            }
            ProductSummaryPersonalizationError::UserStateReadModelInvalid { source } => {
                Self::ProductUserStateReadModelInvalid { source }
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
    use crate::ports::{ProductEmbedding, ProductSimilarProductsReadError};
    use crate::use_cases::{ProductSummary, ProductSummaryPriceValuation};
    use application::{
        error::box_error,
        operation_context::{CorrelationId, Principal, RequestId},
        transaction::TransactionError,
    };
    use domain_primitives::event_id::EventId;
    use fxrate_core::FxRateId;
    use fxrate_core::{FX_RATE_SCALE, FxRateQuote, FxRateSource, NewFxRateSnapshot};
    use indexmap::IndexSet;
    use localization::Localized;
    use money::{Currency, MonetaryAmount, Price};
    use product_listing_core::{
        product_id::ProductId, product_lifecycle::ProductLifecycle, product_slug_id::ProductSlugId,
        product_state::ProductState, shops_product_id::ShopsProductId,
    };
    use shop_core::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};

    use crate::user_state::ProductUserState;
    use product_listing_core::title::Title;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, MutexGuard};
    use strum::IntoEnumIterator;
    use time::OffsetDateTime;
    use url::Url;

    #[derive(Debug, Default)]
    struct FakeState {
        begin_error: bool,
        commit_error: bool,
        find_embedding_result: Option<Result<Option<ProductEmbedding>, ProductEmbeddingReadError>>,
        find_similar_products_result:
            Option<Result<Vec<ProductSummary>, ProductSimilarProductsReadError>>,
        requested_product_ids: Vec<ProductId>,
        requested_similar_products: Vec<ProductSimilarProductsRequest>,
        commit_count: usize,
    }

    type SharedState = Arc<Mutex<FakeState>>;

    #[derive(Clone)]
    struct FakeUnitOfWork {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeEmbeddingReaderFactory {
        state: SharedState,
    }

    struct FakeTx {
        state: SharedState,
    }

    struct FakeEmbeddingReader {
        state: SharedState,
    }

    #[derive(Clone, Copy)]
    struct FakeFxRateSnapshotRepositoryFactory;

    struct FakeFxRateSnapshotRepository;

    #[derive(Clone)]
    struct FakeSimilarProductsReader {
        state: SharedState,
    }

    #[derive(Clone, Copy)]
    struct EmptyUserStateReader;

    #[derive(Clone)]
    struct StaticUserStateReader {
        states: HashMap<ProductId, ProductUserState>,
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

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            if lock_state(&self.state).begin_error {
                Err(TransactionError::BeginFailed)
            } else {
                Ok(FakeTx {
                    state: Arc::clone(&self.state),
                })
            }
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
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
            FakeFxRateSnapshotRepository
        }
    }

    #[async_trait::async_trait]
    impl FxRateSnapshotRepository for FakeFxRateSnapshotRepository {
        async fn find_latest(
            &mut self,
        ) -> Result<Option<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(Some(test_fx_snapshot()))
        }

        async fn find_latest_at_or_before(
            &mut self,
            _timestamp: OffsetDateTime,
        ) -> Result<Option<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(Some(test_fx_snapshot()))
        }

        async fn find_by_id(
            &mut self,
            _id: FxRateId,
        ) -> Result<Option<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(Some(test_fx_snapshot()))
        }

        async fn find_by_ids(
            &mut self,
            _ids: &[FxRateId],
        ) -> Result<Vec<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(vec![test_fx_snapshot()])
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

    fn test_fx_snapshot() -> fxrate_core::FxRateSnapshot {
        NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| FxRateQuote::new(currency, FX_RATE_SCALE)),
        )
        .and_then(|snapshot| Ok(snapshot.into_persisted(1_i64.try_into()?)))
        .unwrap_or_else(|error| panic!("test FX snapshot must be valid: {error}"))
    }

    impl ProductEmbeddingReaderFactory<FakeTx> for FakeEmbeddingReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl ProductEmbeddingReader + 'tx {
            FakeEmbeddingReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductEmbeddingReader for FakeEmbeddingReader {
        async fn find_embedding(
            &mut self,
            lookup: &ProductEmbeddingLookup,
        ) -> Result<Option<ProductEmbedding>, ProductEmbeddingReadError> {
            let product_id = match lookup {
                ProductEmbeddingLookup::ById(product_id) => *product_id,
                ProductEmbeddingLookup::BySlug { .. } => ProductId::new(),
            };
            let mut state = lock_state(&self.state);
            state.requested_product_ids.push(product_id);
            match state.find_embedding_result.take() {
                Some(result) => result,
                None => Ok(None),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductUserStateReader for EmptyUserStateReader {
        async fn find_for_user(
            &self,
            _lookup: &crate::ports::ProductUserStateLookup,
        ) -> Result<HashMap<ProductId, ProductUserState>, crate::ports::ProductUserStateReadError>
        {
            Ok(HashMap::new())
        }
    }

    #[async_trait::async_trait]
    impl ProductUserStateReader for StaticUserStateReader {
        async fn find_for_user(
            &self,
            _lookup: &crate::ports::ProductUserStateLookup,
        ) -> Result<HashMap<ProductId, ProductUserState>, crate::ports::ProductUserStateReadError>
        {
            Ok(self.states.clone())
        }
    }

    #[async_trait::async_trait]
    impl ProductSimilarProductsReader for FakeSimilarProductsReader {
        async fn find_similar_products(
            &self,
            request: &ProductSimilarProductsRequest,
        ) -> Result<Vec<ProductSummary>, ProductSimilarProductsReadError> {
            let mut state = lock_state(&self.state);
            state.requested_similar_products.push(request.clone());
            match state.find_similar_products_result.take() {
                Some(result) => result,
                None => Ok(Vec::new()),
            }
        }
    }

    fn handler(
        state: &SharedState,
    ) -> GetSimilarProductsHandler<
        FakeUnitOfWork,
        FakeEmbeddingReaderFactory,
        FakeFxRateSnapshotRepositoryFactory,
        FakeSimilarProductsReader,
        EmptyUserStateReader,
    > {
        GetSimilarProductsHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(state),
            },
            FakeEmbeddingReaderFactory {
                state: Arc::clone(state),
            },
            FakeFxRateSnapshotRepositoryFactory,
            FakeSimilarProductsReader {
                state: Arc::clone(state),
            },
            EmptyUserStateReader,
        )
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn authenticated_context(user_id: user_core::user_id::UserId) -> OperationContext {
        OperationContext {
            principal: Principal::User(user_id),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn product_summary(product_id: ProductId) -> Result<ProductSummary, url::ParseError> {
        Ok(ProductSummary {
            product_id,
            product_slug_id: ProductSlugId::from("cabinet-abcdef"),
            event_id: EventId::new(),
            shop_id: ShopId::new(),
            seller_id: ShopId::new(),
            shops_product_id: ShopsProductId::new(),
            shop_name: ShopName::from("Shop"),
            shop_slug_id: ShopSlugId::from("shop"),
            title: Some(Localized::new(Language::En, Title::from("Cabinet"))),
            display_price: Some(Price::new(MonetaryAmount::from(100_u64), Currency::Eur)),
            price_valuation: ProductSummaryPriceValuation::Current {
                fx_rate_id: FxRateId::new(),
                captured_at: OffsetDateTime::UNIX_EPOCH,
            },
            state: ProductState::Listed,
            lifecycle: ProductLifecycle::Active,
            url: Url::parse("https://shop.example/products/1")?,
            view_url: Url::parse("https://aura.example/products/cabinet-abcdef")?,
            images: IndexSet::new(),
            updated: OffsetDateTime::UNIX_EPOCH,
        })
    }

    fn request() -> GetSimilarProductsRequest {
        GetSimilarProductsRequest {
            lookup: ProductEmbeddingLookup::ById(ProductId::new()),
            language: Language::En,
            currency: Currency::Eur,
        }
    }

    #[tokio::test]
    async fn should_return_embedding_pending_when_product_embedding_is_missing() {
        let state = state();
        let request = request();
        lock_state(&state).find_embedding_result = Some(Ok(Some(ProductEmbedding {
            product_id: ProductId::new(),
            embedding: None,
        })));

        let result = handler(&state).execute(&context(), request.clone()).await;

        assert!(matches!(
            result,
            Ok(GetSimilarProductsResult::EmbeddingPending)
        ));
        assert_eq!(
            vec![match request.lookup {
                ProductEmbeddingLookup::ById(product_id) => product_id,
                ProductEmbeddingLookup::BySlug { .. } => ProductId::new(),
            }],
            lock_state(&state).requested_product_ids
        );
        assert_eq!(1, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_return_not_found_when_product_id_is_missing() {
        let state = state();

        let result = handler(&state).execute(&context(), request()).await;

        assert!(matches!(result, Err(GetSimilarProductsError::NotFound)));
        assert_eq!(0, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_return_ready_products_when_embedding_is_available() {
        let state = state();
        let product_id = ProductId::new();
        lock_state(&state).find_embedding_result = Some(Ok(Some(ProductEmbedding {
            product_id,
            embedding: Some(vec![0.1_f32]),
        })));

        let result = handler(&state).execute(&context(), request()).await;

        assert!(
            matches!(result, Ok(GetSimilarProductsResult::Ready(products)) if products.is_empty())
        );
        let state = lock_state(&state);
        assert_eq!(1, state.commit_count);
        assert_eq!(1, state.requested_similar_products.len());
        assert_eq!(product_id, state.requested_similar_products[0].product_id);
        assert_eq!(vec![0.1_f32], state.requested_similar_products[0].embedding);
        assert_eq!(Language::En, state.requested_similar_products[0].language);
    }

    #[tokio::test]
    async fn should_hydrate_ready_similar_products_for_authenticated_user()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let user_id = user_core::user_id::UserId::new();
        let product_id = ProductId::new();
        let mut user_state = ProductUserState::default();
        user_state.watchlist.watching = true;
        lock_state(&state).find_embedding_result = Some(Ok(Some(ProductEmbedding {
            product_id: ProductId::new(),
            embedding: Some(vec![0.1_f32]),
        })));
        lock_state(&state).find_similar_products_result =
            Some(Ok(vec![product_summary(product_id)?]));
        let handler = GetSimilarProductsHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(&state),
            },
            FakeEmbeddingReaderFactory {
                state: Arc::clone(&state),
            },
            FakeFxRateSnapshotRepositoryFactory,
            FakeSimilarProductsReader {
                state: Arc::clone(&state),
            },
            StaticUserStateReader {
                states: HashMap::from([(product_id, user_state)]),
            },
        );

        let result = handler
            .execute(&authenticated_context(user_id), request())
            .await?;

        let GetSimilarProductsResult::Ready(products) = result else {
            return Err(std::io::Error::other("expected ready similar products").into());
        };
        assert_eq!(
            Some(true),
            products[0]
                .user_state
                .as_ref()
                .map(|state| state.watchlist.watching)
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_map_similar_products_reader_failure_to_unavailable_after_commit() {
        let state = state();
        lock_state(&state).find_embedding_result = Some(Ok(Some(ProductEmbedding {
            product_id: ProductId::new(),
            embedding: Some(vec![0.1_f32]),
        })));
        lock_state(&state).find_similar_products_result = Some(Err(
            ProductSimilarProductsReadError::SimilarProductsQueryFailed {
                source: box_error(std::io::Error::other("boom")),
            },
        ));

        let result = handler(&state).execute(&context(), request()).await;

        assert!(matches!(
            result,
            Err(GetSimilarProductsError::SimilaritySearchUnavailable)
        ));
        assert_eq!(1, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_map_embedding_query_failure() {
        let state = state();
        lock_state(&state).find_embedding_result = Some(Err(
            ProductEmbeddingReadError::ProductEmbeddingQueryFailed {
                source: box_error(std::io::Error::other("boom")),
            },
        ));

        let result = handler(&state).execute(&context(), request()).await;

        assert!(matches!(
            result,
            Err(GetSimilarProductsError::ProductEmbeddingQueryFailed { .. })
        ));
        assert_eq!(0, lock_state(&state).commit_count);
    }
}
