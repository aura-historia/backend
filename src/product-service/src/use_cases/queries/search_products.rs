use crate::ports::{ProductSearchReadError, ProductSearchReader, ProductUserStateReader};
use crate::use_cases::queries::product_summary_personalization::{
    ProductSummaryPersonalizationError, hydrate_product_summaries,
};
use common::error::boxed::BoxError;
use common::event_id::EventId;
use common::language::domain::Language;
use common::localized::Localized;
use common::operation_context::{OperationContext, Principal};
use common::pagination::cursor::{Cursor, CursoredResult};
use common::personalized::Personalized;
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
use embedding::{EmbeddingGenerator, EmbeddingText};

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

pub type PersonalizedProductSummary = Personalized<ProductSummary, ProductUserState>;
pub type ProductSearchReadResult = CursoredResult<ProductSummary, Value>;
pub type SearchProductsResult = CursoredResult<PersonalizedProductSummary, Value>;

#[derive(Debug, thiserror::Error)]
pub enum SearchProductsError {
    #[error("product search query failed")]
    ProductSearchQueryFailed,
    #[error("product search read model is invalid")]
    ProductSearchReadModelInvalid,
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

pub struct SearchProductsHandler<R, E, U, N> {
    reader: R,
    embeddings: E,
    user_states: U,
    notifications: N,
}

impl<R, E, U, N> SearchProductsHandler<R, E, U, N> {
    pub fn new(reader: R, embeddings: E, user_states: U, notifications: N) -> Self {
        Self {
            reader,
            embeddings,
            user_states,
            notifications,
        }
    }
}

#[async_trait::async_trait]
impl<R, E, U, N> SearchProductsUseCase for SearchProductsHandler<R, E, U, N>
where
    R: ProductSearchReader,
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
        let result = match hybrid_embedding_query(&request) {
            Some(query) => match self.embeddings.embed_search_query(&query).await {
                Ok(embedding) => {
                    self.reader
                        .search_hybrid(&request, embedding.values())
                        .await?
                }
                Err(_) => self.reader.search(&request).await?,
            },
            None => self.reader.search(&request).await?,
        };
        let mut result = result.map_item(|item| Personalized {
            item,
            user_state: None,
        });
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
    use common::currency::domain::Currency;
    use common::error::boxed::box_error;
    use common::event_id::EventId;
    use common::language::domain::Language;
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use common::price::domain::MonetaryAmount;
    use common::user_id::UserId;
    use embedding::{EmbeddingError, EmbeddingVector};

    use notification_core::notification::{NotificationPayload, NotificationWatchlistPayload};
    use notification_core::notification_id::NotificationId;
    use notification_service::ports::all_notifications_reader::{
        AllNotificationsReadError, AllNotificationsReadItem, AllNotificationsReader,
    };
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, MutexGuard};

    #[derive(Debug, Default)]
    struct FakeState {
        search_result: Option<Result<ProductSearchReadResult, ProductSearchReadError>>,
        hybrid_search_result: Option<Result<ProductSearchReadResult, ProductSearchReadError>>,
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
            _request: &SearchProductsRequest,
        ) -> Result<ProductSearchReadResult, ProductSearchReadError> {
            let mut state = lock_state(&self.state);
            match state.search_result.take() {
                Some(result) => result,
                None => Ok(CursoredResult::default()),
            }
        }

        async fn search_hybrid(
            &self,
            _request: &SearchProductsRequest,
            _embedding: &[f32],
        ) -> Result<ProductSearchReadResult, ProductSearchReadError> {
            let mut state = lock_state(&self.state);
            state.used_hybrid_search = true;
            match state.hybrid_search_result.take() {
                Some(result) => result,
                None => Ok(CursoredResult::default()),
            }
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
        FakeSearchReader,
        FakeEmbeddingGenerator,
        FakeUserStatesReader,
        FakeNotificationsReader,
    > {
        SearchProductsHandler::new(
            search_reader(state),
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

    fn request_with_text_query() -> Result<SearchProductsRequest, Box<dyn std::error::Error>> {
        Ok(SearchProductsRequest {
            search: ProductSearch::new(Language::En, Currency::Eur)
                .with_product_query("vintage brass lamp".try_into()?),
            sort: None,
            cursor: None,
        })
    }

    #[tokio::test]
    async fn should_search_products_when_reader_succeeds() -> Result<(), url::ParseError> {
        let state = state();
        let expected = search_result()?;
        lock_state(&state).search_result = Some(Ok(expected.clone()));

        let result = handler(&state).execute(&context(), request()).await;

        let expected = expected.map_item(|item| Personalized {
            item,
            user_state: None,
        });
        assert!(matches!(result, Ok(actual) if actual == expected));
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

        assert_eq!(
            expected.map_item(|item| Personalized {
                item,
                user_state: None
            }),
            result
        );
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

        assert_eq!(
            expected.map_item(|item| Personalized {
                item,
                user_state: None
            }),
            result
        );
        let state = lock_state(&state);
        assert!(!state.used_hybrid_search);
        assert_eq!(1, state.embedding_queries.len());
        Ok(())
    }

    #[tokio::test]
    async fn should_skip_embedding_for_non_score_sort() -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let expected = search_result()?;
        lock_state(&state).search_result = Some(Ok(expected.clone()));
        let mut request = request_with_text_query()?;
        request.sort = Some(Sort {
            sort: SortProductField::Price,
            order: common::sort::SortOrder::Asc,
        });

        let result = handler(&state).execute(&context(), request).await?;

        assert_eq!(
            expected.map_item(|item| Personalized {
                item,
                user_state: None
            }),
            result
        );
        let state = lock_state(&state);
        assert!(state.embedding_queries.is_empty());
        assert!(!state.used_hybrid_search);
        Ok(())
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
