use crate::ports::{
    SearchFilterMatchListQuery, SearchFilterMatchReadError, SearchFilterMatchReader,
};
use common::error::boxed::{BoxError, box_error, static_error};
use common::language::domain::Language;
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use common::pagination::cursor::{Cursor, CursoredResult};
use common::product_id::ProductId;

use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use notification_service::ports::all_notifications_reader::{
    AllNotificationsReadError, AllNotificationsReadItem, AllNotificationsReader,
};
use product_core::user_state::NotificationUserState;
use product_service::ports::{
    ProductDetailsBatchReadError, ProductDetailsBatchReadRequest, ProductDetailsBatchReader,
};
use product_service::use_cases::{PersonalizedProductDetailsView, redact_hidden_product};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
pub struct ListSearchFilterMatchesRequest {
    pub user_id: UserId,
    pub search_filter_id: UserSearchFilterId,
    pub language: Language,
    pub cursor: Option<Cursor<crate::ports::SearchFilterMatchCursor>>,
    pub order: common::sort::SortOrder,
}

pub type ListSearchFilterMatchesResult =
    CursoredResult<PersonalizedProductDetailsView, crate::ports::SearchFilterMatchCursor>;

#[derive(Debug, thiserror::Error)]
pub enum ListSearchFilterMatchesError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("actor may not manage this search filter")]
    ActorMayNotManageSearchFilter,
    #[error("search filter not found")]
    SearchFilterNotFound,
    #[error("search filter match read failed")]
    SearchFilterMatchReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("matched product details read failed")]
    ProductDetailsReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("matched product details are invalid")]
    ProductDetailsInvalid {
        #[source]
        source: BoxError,
    },
    #[error("matched product is missing from the product details read")]
    MatchedProductMissing { product_id: ProductId },
    #[error("matched product notification read failed")]
    NotificationReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("matched product could not be redacted")]
    HiddenProductRedactionFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ListSearchFilterMatchesUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListSearchFilterMatchesRequest,
    ) -> Result<ListSearchFilterMatchesResult, ListSearchFilterMatchesError>;
}

pub struct ListSearchFilterMatchesHandler<M, P, N> {
    matches: M,
    products: P,
    notifications: N,
}

impl<M, P, N> ListSearchFilterMatchesHandler<M, P, N> {
    pub fn new(matches: M, products: P, notifications: N) -> Self {
        Self {
            matches,
            products,
            notifications,
        }
    }
}

#[async_trait::async_trait]
impl<M, P, N> ListSearchFilterMatchesUseCase for ListSearchFilterMatchesHandler<M, P, N>
where
    M: SearchFilterMatchReader,
    P: ProductDetailsBatchReader,
    N: AllNotificationsReader,
{
    #[tracing::instrument(
        name = "list_search_filter_matches",
        skip_all,
        fields(
            search_filter_id = %request.search_filter_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListSearchFilterMatchesRequest,
    ) -> Result<ListSearchFilterMatchesResult, ListSearchFilterMatchesError> {
        authorize_owner(context, request.user_id)?;
        let matches = self
            .matches
            .list_for_owned_filter(&SearchFilterMatchListQuery {
                user_id: request.user_id,
                search_filter_id: request.search_filter_id,
                cursor: request.cursor,
                order: request.order,
            })
            .await
            .map_err(read_error)?
            .ok_or(ListSearchFilterMatchesError::SearchFilterNotFound)?;

        if matches.items.is_empty() {
            return Ok(CursoredResult {
                items: Vec::new(),
                cursor: matches.cursor,
                total: matches.total,
            });
        }

        let product_ids = matches
            .items
            .iter()
            .map(|matched| matched.product_id)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let details_future = async {
            self.products
                .find_for_user(&ProductDetailsBatchReadRequest {
                    user_id: request.user_id,
                    language: request.language,
                    product_ids,
                    search_filter_id: request.search_filter_id,
                })
                .await
                .map_err(HydrationError::from)
        };
        let notifications_future = async {
            self.notifications
                .list_all_by_user(&request.user_id)
                .await
                .map_err(HydrationError::from)
        };
        let (details, notifications) =
            tokio::try_join!(details_future, notifications_future).map_err(hydration_error)?;
        let newest_notifications = newest_notifications_by_product(notifications);

        let products = matches
            .items
            .into_iter()
            .map(|matched| {
                let mut product = details.get(&matched.product_id).cloned().ok_or(
                    ListSearchFilterMatchesError::MatchedProductMissing {
                        product_id: matched.product_id,
                    },
                )?;
                let user_state = product.user_state.as_mut().ok_or(
                    ListSearchFilterMatchesError::ProductDetailsInvalid {
                        source: static_error("matched product is missing user state"),
                    },
                )?;
                user_state.notification = newest_notifications
                    .get(&matched.product_id)
                    .copied()
                    .unwrap_or_default();
                if user_state.search_filter.hidden {
                    redact_hidden_product(&mut product.item).map_err(|error| {
                        ListSearchFilterMatchesError::HiddenProductRedactionFailed {
                            source: box_error(error),
                        }
                    })?;
                }
                Ok(product)
            })
            .collect::<Result<Vec<_>, ListSearchFilterMatchesError>>()?;

        Ok(CursoredResult {
            items: products,
            cursor: matches.cursor,
            total: matches.total,
        })
    }
}

fn newest_notifications_by_product(
    notifications: Vec<AllNotificationsReadItem>,
) -> HashMap<ProductId, NotificationUserState> {
    let mut newest = HashMap::new();

    for notification in notifications {
        let Some(product_id) = notification.product_id() else {
            continue;
        };
        let state = NotificationUserState {
            seen: notification.seen,
            origin_event_id: Some(notification.origin_event_id),
        };
        let replace = newest
            .get(&product_id)
            .and_then(|current: &NotificationUserState| current.origin_event_id)
            .is_none_or(|current_event_id| notification.origin_event_id > current_event_id);
        if replace {
            newest.insert(product_id, state);
        }
    }

    newest
}

fn authorize_owner(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), ListSearchFilterMatchesError> {
    context
        .require()
        .credential_capability(CredentialCapability::SearchFiltersWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<ListSearchFilterMatchesError>()
}

fn read_error(error: SearchFilterMatchReadError) -> ListSearchFilterMatchesError {
    ListSearchFilterMatchesError::SearchFilterMatchReadFailed {
        source: box_error(error),
    }
}

fn hydration_error(error: HydrationError) -> ListSearchFilterMatchesError {
    match error {
        HydrationError::Details(ProductDetailsBatchReadError::QueryFailed { source }) => {
            ListSearchFilterMatchesError::ProductDetailsReadFailed { source }
        }
        HydrationError::Details(ProductDetailsBatchReadError::InvalidReadModel { source }) => {
            ListSearchFilterMatchesError::ProductDetailsInvalid { source }
        }
        HydrationError::Notifications(error) => {
            ListSearchFilterMatchesError::NotificationReadFailed {
                source: box_error(error),
            }
        }
    }
}

enum HydrationError {
    Details(ProductDetailsBatchReadError),
    Notifications(AllNotificationsReadError),
}

impl From<ProductDetailsBatchReadError> for HydrationError {
    fn from(error: ProductDetailsBatchReadError) -> Self {
        Self::Details(error)
    }
}

impl From<AllNotificationsReadError> for HydrationError {
    fn from(error: AllNotificationsReadError) -> Self {
        Self::Notifications(error)
    }
}

impl From<OperationAuthorizationError> for ListSearchFilterMatchesError {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        SearchFilterMatchCursor, SearchFilterMatchListItem, SearchFilterMatchReadError,
    };
    use common::event_id::EventId;
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use common::personalized::Personalized;
    use common::product_lifecycle::domain::ProductLifecycle;
    use common::product_slug_id::ProductSlugId;
    use common::product_state::domain::ProductState;
    use common::shop_id::ShopId;
    use common::shop_name::ShopName;
    use common::shop_slug_id::ShopSlugId;
    use common::shops_product_id::ShopsProductId;
    use indexmap::IndexSet;
    use product_core::product::{ProductAddress, ProductAuction, ProductPricing};
    use product_core::user_state::ProductUserState;
    use product_service::use_cases::ProductDetailsView;
    use std::sync::{Arc, Mutex, MutexGuard};
    use time::OffsetDateTime;
    use url::Url;

    #[derive(Default)]
    struct State {
        product_requests: Vec<ProductDetailsBatchReadRequest>,
        notification_requests: usize,
    }

    type SharedState = Arc<Mutex<State>>;

    fn lock(state: &SharedState) -> MutexGuard<'_, State> {
        match state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    struct MatchesReader {
        matches: Vec<SearchFilterMatchListItem>,
    }

    #[async_trait::async_trait]
    impl SearchFilterMatchReader for MatchesReader {
        async fn list_for_owned_filter(
            &self,
            _query: &SearchFilterMatchListQuery,
        ) -> Result<
            Option<CursoredResult<SearchFilterMatchListItem, SearchFilterMatchCursor>>,
            SearchFilterMatchReadError,
        > {
            Ok(Some(CursoredResult {
                items: self.matches.clone(),
                cursor: Cursor::default(),
                total: None,
            }))
        }
    }

    struct ProductsReader {
        state: SharedState,
        products: HashMap<ProductId, PersonalizedProductDetailsView>,
    }

    #[async_trait::async_trait]
    impl ProductDetailsBatchReader for ProductsReader {
        async fn find_for_user(
            &self,
            request: &ProductDetailsBatchReadRequest,
        ) -> Result<HashMap<ProductId, PersonalizedProductDetailsView>, ProductDetailsBatchReadError>
        {
            lock(&self.state).product_requests.push(request.clone());
            Ok(self.products.clone())
        }
    }

    struct NotificationsReader(SharedState);

    #[async_trait::async_trait]
    impl AllNotificationsReader for NotificationsReader {
        async fn list_all_by_user(
            &self,
            _user_id: &UserId,
        ) -> Result<Vec<AllNotificationsReadItem>, AllNotificationsReadError> {
            lock(&self.0).notification_requests += 1;
            Ok(Vec::new())
        }
    }

    fn product(product_id: ProductId) -> Result<PersonalizedProductDetailsView, url::ParseError> {
        let url = Url::parse("https://example.test/product")?;
        Ok(Personalized {
            item: ProductDetailsView {
                product_id,
                product_slug_id: ProductSlugId::from("product"),
                event_id: EventId::new(),
                shop_id: ShopId::new(),
                seller_id: ShopId::new(),
                shops_product_id: ShopsProductId::from("product"),
                shop_name: ShopName::from("Shop"),
                seller_name: ShopName::from("Seller"),
                shop_slug_id: ShopSlugId::from("shop"),
                seller_slug_id: ShopSlugId::from("seller"),
                address: ProductAddress::default(),
                product_title: None,
                product_description: None,
                title: None,
                description: None,
                pricing: ProductPricing::default(),
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
                currency: None,
                state: ProductState::Available,
                lifecycle: ProductLifecycle::Active,
                url: url.clone(),
                view_url: url,
                images: IndexSet::new(),
                auction: ProductAuction::default(),
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            },
            user_state: Some(ProductUserState::default()),
        })
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    #[tokio::test]
    async fn should_batch_hydrate_matches_and_preserve_match_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let first = ProductId::new();
        let second = ProductId::new();
        let state = Arc::new(Mutex::new(State::default()));
        let handler = ListSearchFilterMatchesHandler::new(
            MatchesReader {
                matches: vec![
                    SearchFilterMatchListItem {
                        product_id: first,
                        created: OffsetDateTime::UNIX_EPOCH,
                    },
                    SearchFilterMatchListItem {
                        product_id: second,
                        created: OffsetDateTime::UNIX_EPOCH,
                    },
                ],
            },
            ProductsReader {
                state: Arc::clone(&state),
                products: HashMap::from([(second, product(second)?), (first, product(first)?)]),
            },
            NotificationsReader(Arc::clone(&state)),
        );

        let result = handler
            .execute(
                &context(),
                ListSearchFilterMatchesRequest {
                    user_id: UserId::new(),
                    search_filter_id: UserSearchFilterId::new(),
                    language: Language::En,
                    cursor: None,
                    order: common::sort::SortOrder::Asc,
                },
            )
            .await?;

        assert_eq!(
            vec![first, second],
            result
                .items
                .iter()
                .map(|item| item.item.product_id)
                .collect::<Vec<_>>()
        );
        let state = lock(&state);
        assert_eq!(1, state.product_requests.len());
        assert_eq!(1, state.notification_requests);
        assert_eq!(
            HashSet::from([first, second]),
            state.product_requests[0]
                .product_ids
                .iter()
                .copied()
                .collect::<HashSet<_>>()
        );
        Ok(())
    }
}
