use crate::ports::{ProductSearchReadError, ProductSearchReader};
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
}

#[async_trait::async_trait]
pub trait SearchProductsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchProductsRequest,
    ) -> Result<SearchProductsResult, SearchProductsError>;
}

pub struct SearchProductsHandler<R> {
    reader: R,
}

impl<R> SearchProductsHandler<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
    }
}

#[async_trait::async_trait]
impl<R> SearchProductsUseCase for SearchProductsHandler<R>
where
    R: ProductSearchReader,
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
        self.reader.search(&request).await.map_err(Into::into)
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
    use std::sync::{Arc, Mutex, MutexGuard};

    #[derive(Debug, Default)]
    struct FakeState {
        search_result: Option<Result<SearchProductsResult, ProductSearchReadError>>,
    }

    type SharedState = Arc<Mutex<FakeState>>;

    #[derive(Clone)]
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

    fn search_reader(state: &SharedState) -> FakeSearchReader {
        FakeSearchReader {
            state: Arc::clone(state),
        }
    }

    #[async_trait::async_trait]
    impl ProductSearchReader for FakeSearchReader {
        async fn search(
            &self,
            _request: &SearchProductsRequest,
        ) -> Result<SearchProductsResult, ProductSearchReadError> {
            let mut state = lock_state(&self.state);
            match state.search_result.take() {
                Some(result) => result,
                None => Ok(CursoredResult::default()),
            }
        }
    }

    fn handler(state: &SharedState) -> SearchProductsHandler<FakeSearchReader> {
        SearchProductsHandler::new(search_reader(state))
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
