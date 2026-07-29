use crate::ports::{ProductSearchReadError, ProductSearchReader, ProductSearchReaderFactory};
use common::event_id::EventId;
use common::language::domain::Language;
use common::localized::Localized;
use common::operation_context::OperationContext;
use common::pagination::cursor::{Cursor, CursoredResult};
use common::price::domain::Price;
use common::product_id::ProductId;
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_slug_id::ProductSlugId;
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::shops_product_id::ShopsProductId;
use common::sort::Sort;
use common::transaction::{Transaction, UnitOfWork};
use indexmap::IndexSet;
use product_core::product_image::ProductImage;
use product_core::product_search::ProductSearch;
use product_core::sort_product_field::SortProductField;
use product_core::title::Title;
use serde_json::Value;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchProductsRequest {
    pub search: ProductSearch,
    pub sort: Option<Sort<SortProductField>>,
    pub cursor: Option<Cursor<Value>>,
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
    pub price: Option<Price>,
    pub state: ProductState,
    pub lifecycle: ProductLifecycle,
    pub url: Url,
    pub view_url: Url,
    pub images: IndexSet<ProductImage>,
    pub updated: OffsetDateTime,
}

pub type SearchProductsResult = CursoredResult<ProductSummary, Value>;

#[derive(Debug, thiserror::Error)]
pub enum SearchProductsError {
    #[error("product search query failed")]
    ProductSearchQueryFailed,
    #[error("product search read model is invalid")]
    ProductSearchReadModelInvalid,
    #[error("failed to begin search products transaction")]
    BeginTransactionFailed,
    #[error("failed to commit search products transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait SearchProductsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchProductsRequest,
    ) -> Result<SearchProductsResult, SearchProductsError>;
}

pub struct SearchProductsHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> SearchProductsHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> SearchProductsUseCase for SearchProductsHandler<U, R>
where
    U: UnitOfWork,
    R: ProductSearchReaderFactory<U::Tx>,
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
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| SearchProductsError::BeginTransactionFailed)?;
        let result = self.reader.in_transaction(&mut tx).search(&request).await?;
        tx.commit()
            .await
            .map_err(|_| SearchProductsError::CommitTransactionFailed)?;

        Ok(result)
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

#[cfg(test)]
mod tests {
    use super::*;
    use common::currency::domain::Currency;
    use common::language::domain::Language;
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use common::price::domain::MonetaryAmount;
    use common::transaction::TransactionError;
    use std::sync::{Arc, Mutex, MutexGuard};

    #[derive(Debug, Default)]
    struct FakeState {
        begin_error: bool,
        commit_error: bool,
        search_result: Option<Result<SearchProductsResult, ProductSearchReadError>>,
        commit_count: usize,
    }

    type SharedState = Arc<Mutex<FakeState>>;

    #[derive(Clone)]
    struct FakeUnitOfWork {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeSearchReaderFactory {
        state: SharedState,
    }

    struct FakeTx {
        state: SharedState,
    }

    struct FakeSearchReader {
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

    fn uow(state: &SharedState) -> FakeUnitOfWork {
        FakeUnitOfWork {
            state: Arc::clone(state),
        }
    }

    fn search_reader_factory(state: &SharedState) -> FakeSearchReaderFactory {
        FakeSearchReaderFactory {
            state: Arc::clone(state),
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let state = lock_state(&self.state);
            if state.begin_error {
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

    impl ProductSearchReaderFactory<FakeTx> for FakeSearchReaderFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl ProductSearchReader + 'tx {
            FakeSearchReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductSearchReader for FakeSearchReader {
        async fn search(
            &mut self,
            _request: &SearchProductsRequest,
        ) -> Result<SearchProductsResult, ProductSearchReadError> {
            let mut state = lock_state(&self.state);
            match state.search_result.take() {
                Some(result) => result,
                None => Ok(CursoredResult::default()),
            }
        }
    }

    fn handler(
        state: &SharedState,
    ) -> SearchProductsHandler<FakeUnitOfWork, FakeSearchReaderFactory> {
        SearchProductsHandler::new(uow(state), search_reader_factory(state))
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn search_result() -> Result<SearchProductsResult, url::ParseError> {
        Ok(SearchProductsResult {
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
                price: Some(Price::new(MonetaryAmount::from(100_u64), Currency::Eur)),
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

    fn request() -> SearchProductsRequest {
        SearchProductsRequest {
            search: ProductSearch::new(Language::En, Currency::Eur),
            sort: None,
            cursor: None,
        }
    }

    #[tokio::test]
    async fn should_search_products_when_reader_succeeds() -> Result<(), url::ParseError> {
        let state = state();
        let expected = search_result()?;
        lock_state(&state).search_result = Some(Ok(expected.clone()));

        let result = handler(&state).execute(&context(), request()).await;

        assert!(matches!(result, Ok(actual) if actual == expected));
        assert_eq!(1, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_map_begin_error_when_search_products_begin_fails() {
        let state = state();
        lock_state(&state).begin_error = true;

        let result = handler(&state).execute(&context(), request()).await;

        assert!(matches!(
            result,
            Err(SearchProductsError::BeginTransactionFailed)
        ));
    }

    #[tokio::test]
    async fn should_map_commit_error_when_search_products_commit_fails()
    -> Result<(), url::ParseError> {
        let state = state();
        lock_state(&state).commit_error = true;
        lock_state(&state).search_result = Some(Ok(search_result()?));

        let result = handler(&state).execute(&context(), request()).await;

        assert!(matches!(
            result,
            Err(SearchProductsError::CommitTransactionFailed)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_when_search_products_read_fails() {
        let state = state();
        lock_state(&state).search_result =
            Some(Err(ProductSearchReadError::ProductSearchQueryFailed));

        let result = handler(&state).execute(&context(), request()).await;

        assert!(matches!(
            result,
            Err(SearchProductsError::ProductSearchQueryFailed)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
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
