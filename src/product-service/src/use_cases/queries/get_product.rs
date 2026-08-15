use crate::ports::{
    ProductDetailsReadError, ProductDetailsReadRequest, ProductDetailsReader,
    ProductDetailsReaderFactory,
};
use common::currency::domain::Currency;
use common::error::boxed::{BoxError, box_error};
use common::event_id::EventId;
use common::language::domain::Language;
use common::localized::Localized;
use common::operation_context::{OperationContext, Principal};
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
use common::transaction::{Transaction, UnitOfWork};
use common::user_id::UserId;
use indexmap::IndexSet;
use notification_service::ports::product_notifications_reader::{
    ProductNotificationsReadError, ProductNotificationsReader,
};
use product_core::description::Description;
use product_core::product::{ProductAddress, ProductAuction, ProductPricing};
use product_core::product_image::ProductImage;
use product_core::title::Title;
use product_core::user_state::{NotificationUserState, ProductUserState};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub enum ProductLookup {
    ById(ProductId),
    BySlug {
        shop_slug_id: ShopSlugId,
        product_slug_id: ProductSlugId,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct GetProductRequest {
    pub lookup: ProductLookup,
    pub language: Language,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductDetailsView {
    pub product_id: ProductId,
    pub product_slug_id: ProductSlugId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub seller_name: ShopName,
    pub shop_slug_id: ShopSlugId,
    pub seller_slug_id: ShopSlugId,
    pub address: ProductAddress,
    pub product_title: Option<Localized<Language, Title>>,
    pub product_description: Option<Localized<Language, Description>>,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub pricing: ProductPricing,
    pub price: Option<Price>,
    pub price_estimate_min: Option<Price>,
    pub price_estimate_max: Option<Price>,
    pub currency: Option<Currency>,
    pub state: ProductState,
    pub lifecycle: ProductLifecycle,
    pub url: Url,
    pub view_url: Url,
    pub images: IndexSet<ProductImage>,
    pub auction: ProductAuction,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

pub type PersonalizedProductDetailsView = Personalized<ProductDetailsView, ProductUserState>;

#[derive(Debug, thiserror::Error)]
pub enum GetProductError {
    #[error("product not found")]
    NotFound,
    #[error("product details query failed")]
    ProductDetailsQueryFailed,
    #[error("product details read model is invalid")]
    ProductDetailsReadModelInvalid,
    #[error("product notification read failed")]
    ProductNotificationReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin get product transaction")]
    BeginTransactionFailed,
    #[error("failed to commit get product transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait GetProductUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetProductRequest,
    ) -> Result<PersonalizedProductDetailsView, GetProductError>;
}

pub struct GetProductHandler<U, D, N> {
    unit_of_work: U,
    details_reader: D,
    product_notifications: N,
}

impl<U, D, N> GetProductHandler<U, D, N> {
    pub fn new(unit_of_work: U, details_reader: D, product_notifications: N) -> Self {
        Self {
            unit_of_work,
            details_reader,
            product_notifications,
        }
    }
}

#[async_trait::async_trait]
impl<U, D, N> GetProductUseCase for GetProductHandler<U, D, N>
where
    U: UnitOfWork,
    D: ProductDetailsReaderFactory<U::Tx>,
    N: ProductNotificationsReader,
{
    #[tracing::instrument(
        name = "get_product",
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
        request: GetProductRequest,
    ) -> Result<PersonalizedProductDetailsView, GetProductError> {
        let user_id = personalization_user_id(&context.principal);
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetProductError::BeginTransactionFailed)?;
        let mut details = self
            .details_reader
            .in_transaction(&mut tx)
            .find_details(&ProductDetailsReadRequest {
                lookup: request.lookup,
                language: request.language,
                user_id,
            })
            .await?
            .ok_or(GetProductError::NotFound)?;

        tx.commit()
            .await
            .map_err(|_| GetProductError::CommitTransactionFailed)?;

        if let Some(user_id) = user_id {
            let user_state = details
                .user_state
                .as_mut()
                .ok_or(GetProductError::ProductDetailsReadModelInvalid)?;
            let notification = self
                .product_notifications
                .list_by_product(&user_id, &details.item.product_id, Some(1), true)
                .await
                .map_err(product_notification_read_error)?
                .into_iter()
                .next()
                .map(|notification| NotificationUserState {
                    seen: notification.seen,
                    origin_event_id: Some(notification.origin_event_id),
                })
                .unwrap_or_default();
            user_state.notification = notification;

            if user_state.search_filter.hidden {
                redact_hidden_product(&mut details.item)?;
            }
        }

        Ok(details)
    }
}

fn personalization_user_id(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}

fn product_notification_read_error(error: ProductNotificationsReadError) -> GetProductError {
    GetProductError::ProductNotificationReadFailed {
        source: box_error(error),
    }
}

pub fn redact_hidden_product(details: &mut ProductDetailsView) -> Result<(), GetProductError> {
    let nil = uuid::Uuid::nil();
    let language = details
        .title
        .as_ref()
        .map(|title| title.localization)
        .unwrap_or(Language::En);
    let hidden_url = Url::parse("https://aura-historia.com/pricing")
        .map_err(|_| GetProductError::ProductDetailsReadModelInvalid)?;

    details.product_id = ProductId::from(nil);
    details.product_slug_id = ProductSlugId::from("Hidden");
    details.event_id = EventId::from(nil);
    details.shop_id = ShopId::from(nil);
    details.seller_id = ShopId::from(nil);
    details.shops_product_id = ShopsProductId::from(nil.to_string());
    details.shop_name = ShopName::from("Hidden");
    details.seller_name = ShopName::from("Hidden");
    details.shop_slug_id = ShopSlugId::from("Hidden");
    details.seller_slug_id = ShopSlugId::from("Hidden");
    details.address = ProductAddress::default();
    details.product_title = None;
    details.product_description = None;
    details.title = Some(Localized::new(language, hidden_title(language)));
    details.description = None;
    details.pricing = ProductPricing::default();
    details.price = None;
    details.price_estimate_min = None;
    details.price_estimate_max = None;
    details.currency = None;
    details.state = ProductState::Unknown;
    details.url = hidden_url.clone();
    details.view_url = hidden_url;
    details.images = IndexSet::new();
    details.auction = ProductAuction::default();
    details.created = OffsetDateTime::UNIX_EPOCH;
    details.updated = OffsetDateTime::UNIX_EPOCH;

    Ok(())
}

fn hidden_title(language: Language) -> Title {
    match language {
        Language::De => Title::from("Versteckter Produkttitel"),
        Language::En => Title::from("Hidden Product Title"),
        Language::Fr => Title::from("Titre du produit masqué"),
        Language::Es => Title::from("Título de producto oculto"),
        Language::It => Title::from("Titolo del prodotto nascosto"),
        _ => Title::from("Hidden Product Title"),
    }
}

impl From<ProductDetailsReadError> for GetProductError {
    fn from(error: ProductDetailsReadError) -> Self {
        match error {
            ProductDetailsReadError::ProductDetailsQueryFailed => Self::ProductDetailsQueryFailed,
            ProductDetailsReadError::ProductDetailsReadModelInvalid => {
                Self::ProductDetailsReadModelInvalid
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use common::price::domain::MonetaryAmount;
    use common::transaction::TransactionError;
    use notification_core::notification::{
        NotificationPartnerApplicationPayload, NotificationPayload,
    };
    use notification_core::notification_id::NotificationId;
    use notification_service::ports::product_notifications_reader::ProductNotificationReadItem;
    use std::sync::{Arc, Mutex, MutexGuard};

    #[derive(Debug, Default)]
    struct FakeState {
        begin_error: bool,
        commit_error: bool,
        find_details_result:
            Option<Result<Option<PersonalizedProductDetailsView>, ProductDetailsReadError>>,
        find_details_request: Option<ProductDetailsReadRequest>,
        notification_result:
            Option<Result<Vec<ProductNotificationReadItem>, ProductNotificationsReadError>>,
        notification_requests: Vec<(UserId, ProductId, Option<i32>, bool)>,
        notification_called_after_commit: Option<bool>,
        commit_count: usize,
    }

    type SharedState = Arc<Mutex<FakeState>>;

    #[derive(Clone)]
    struct FakeUnitOfWork {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeDetailsReaderFactory {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeProductNotificationsReader {
        state: SharedState,
    }

    struct FakeTx {
        state: SharedState,
    }

    struct FakeDetailsReader {
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

    impl ProductDetailsReaderFactory<FakeTx> for FakeDetailsReaderFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl ProductDetailsReader + 'tx {
            FakeDetailsReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductDetailsReader for FakeDetailsReader {
        async fn find_details(
            &mut self,
            request: &ProductDetailsReadRequest,
        ) -> Result<Option<PersonalizedProductDetailsView>, ProductDetailsReadError> {
            let mut state = lock_state(&self.state);
            state.find_details_request = Some(request.clone());
            match state.find_details_result.take() {
                Some(result) => result,
                None => Ok(None),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductNotificationsReader for FakeProductNotificationsReader {
        async fn list_by_product(
            &self,
            user_id: &UserId,
            product_id: &ProductId,
            limit: Option<i32>,
            newest_first: bool,
        ) -> Result<Vec<ProductNotificationReadItem>, ProductNotificationsReadError> {
            let mut state = lock_state(&self.state);
            state
                .notification_requests
                .push((*user_id, *product_id, limit, newest_first));
            state.notification_called_after_commit = Some(state.commit_count == 1);
            match state.notification_result.take() {
                Some(result) => result,
                None => Ok(Vec::new()),
            }
        }
    }

    fn handler(
        state: &SharedState,
    ) -> GetProductHandler<FakeUnitOfWork, FakeDetailsReaderFactory, FakeProductNotificationsReader>
    {
        GetProductHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(state),
            },
            FakeDetailsReaderFactory {
                state: Arc::clone(state),
            },
            FakeProductNotificationsReader {
                state: Arc::clone(state),
            },
        )
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn request(language: Language) -> GetProductRequest {
        GetProductRequest {
            lookup: ProductLookup::ById(ProductId::new()),
            language,
        }
    }

    fn url(value: &str) -> Result<Url, url::ParseError> {
        Url::parse(value)
    }

    fn details_view() -> Result<PersonalizedProductDetailsView, url::ParseError> {
        let product_id = ProductId::new();
        Ok(Personalized {
            item: ProductDetailsView {
                product_id,
                product_slug_id: ProductSlugId::from("cabinet-abcdef"),
                event_id: EventId::new(),
                shop_id: ShopId::new(),
                seller_id: ShopId::new(),
                shops_product_id: ShopsProductId::new(),
                shop_name: ShopName::from("Shop"),
                seller_name: ShopName::from("Seller"),
                shop_slug_id: ShopSlugId::from("shop"),
                seller_slug_id: ShopSlugId::from("seller"),
                address: ProductAddress::default(),
                product_title: Some(Localized::new(Language::En, Title::from("Cabinet"))),
                product_description: Some(Localized::new(
                    Language::En,
                    Description::from("Native"),
                )),
                title: Some(Localized::new(Language::En, Title::from("Cabinet"))),
                description: Some(Localized::new(
                    Language::En,
                    Description::from("Description"),
                )),
                pricing: ProductPricing {
                    price: Some(Price::new(MonetaryAmount::from(100_u64), Currency::Eur)),
                    price_estimate_min: None,
                    price_estimate_max: None,
                },
                price: Some(Price::new(MonetaryAmount::from(100_u64), Currency::Eur)),
                price_estimate_min: None,
                price_estimate_max: None,
                currency: Some(Currency::Eur),
                state: ProductState::Listed,
                lifecycle: ProductLifecycle::Active,
                url: url("https://shop.example/products/1")?,
                view_url: url("https://aura.example/products/cabinet-abcdef")?,
                images: IndexSet::<ProductImage>::new(),
                auction: ProductAuction::default(),
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            },
            user_state: None,
        })
    }

    fn personalized_details_view() -> Result<PersonalizedProductDetailsView, url::ParseError> {
        let mut view = details_view()?;
        view.user_state = Some(ProductUserState::default());
        Ok(view)
    }

    fn notification_item(
        user_id: UserId,
        origin_event_id: EventId,
        seen: bool,
    ) -> ProductNotificationReadItem {
        ProductNotificationReadItem {
            user_id,
            origin_event_id,
            notification_id: NotificationId::new(),
            notification_type: None,
            notification_payload: NotificationPayload::PartnerApplication {
                shop_name: ShopName::from("Shop"),
                image: None,
                partner_application_payload: NotificationPartnerApplicationPayload::Approved {
                    partner_application_id:
                        common::partner_shop_application_id::PartnerShopApplicationId::new(),
                },
            },
            seen,
            external: false,
        }
    }

    #[tokio::test]
    async fn should_return_public_details_without_notification_hydration_for_anonymous_request()
    -> Result<(), url::ParseError> {
        let state = state();
        let view = details_view()?;
        lock_state(&state).find_details_result = Some(Ok(Some(view.clone())));
        let request = request(Language::De);

        let result = handler(&state)
            .execute(&context(Principal::Anonymous), request.clone())
            .await;

        assert!(matches!(result, Ok(actual) if actual == view));
        let state = lock_state(&state);
        assert_eq!(1, state.commit_count);
        assert!(state.notification_requests.is_empty());
        assert_eq!(
            Some(ProductDetailsReadRequest {
                lookup: request.lookup,
                language: Language::De,
                user_id: None,
            }),
            state.find_details_request
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_hydrate_newest_notification_after_postgres_commit_for_authenticated_user()
    -> Result<(), url::ParseError> {
        let state = state();
        let user_id = UserId::new();
        let view = personalized_details_view()?;
        let product_id = view.item.product_id;
        let oldest_event_id = EventId::new();
        let newest_event_id = EventId::new();
        lock_state(&state).find_details_result = Some(Ok(Some(view)));
        lock_state(&state).notification_result = Some(Ok(vec![
            notification_item(user_id, newest_event_id, false),
            notification_item(user_id, oldest_event_id, true),
        ]));

        let result = handler(&state)
            .execute(&context(Principal::User(user_id)), request(Language::En))
            .await;

        match result {
            Ok(view) => {
                let user_state = view.user_state.unwrap_or_default();
                assert!(!user_state.notification.seen);
                assert_eq!(
                    Some(newest_event_id),
                    user_state.notification.origin_event_id
                );
            }
            Err(error) => panic!("expected personalized details: {error}"),
        }
        let state = lock_state(&state);
        assert_eq!(1, state.commit_count);
        assert_eq!(Some(true), state.notification_called_after_commit);
        assert_eq!(
            vec![(user_id, product_id, Some(1), true)],
            state.notification_requests
        );
        assert_eq!(
            Some(user_id),
            state
                .find_details_request
                .as_ref()
                .and_then(|request| request.user_id)
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_keep_default_notification_state_when_no_notification_exists()
    -> Result<(), url::ParseError> {
        let state = state();
        let user_id = UserId::new();
        lock_state(&state).find_details_result = Some(Ok(Some(personalized_details_view()?)));

        let result = handler(&state)
            .execute(
                &context(Principal::DelegatedUser {
                    user_id,
                    capabilities: Default::default(),
                }),
                request(Language::En),
            )
            .await;

        match result {
            Ok(view) => {
                let user_state = view.user_state.unwrap_or_default();
                assert!(user_state.notification.seen);
                assert_eq!(None, user_state.notification.origin_event_id);
            }
            Err(error) => panic!("expected personalized details: {error}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_fail_after_commit_when_notification_hydration_fails()
    -> Result<(), url::ParseError> {
        let state = state();
        let user_id = UserId::new();
        lock_state(&state).find_details_result = Some(Ok(Some(personalized_details_view()?)));
        lock_state(&state).notification_result =
            Some(Err(ProductNotificationsReadError::OperationFailed {
                source: box_error(std::io::Error::other("dynamodb unavailable")),
            }));

        let result = handler(&state)
            .execute(&context(Principal::User(user_id)), request(Language::En))
            .await;

        assert!(matches!(
            result,
            Err(GetProductError::ProductNotificationReadFailed { .. })
        ));
        let state = lock_state(&state);
        assert_eq!(1, state.commit_count);
        assert_eq!(Some(true), state.notification_called_after_commit);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_authenticated_details_without_postgres_user_state()
    -> Result<(), url::ParseError> {
        let state = state();
        lock_state(&state).find_details_result = Some(Ok(Some(details_view()?)));

        let result = handler(&state)
            .execute(
                &context(Principal::User(UserId::new())),
                request(Language::En),
            )
            .await;

        assert!(matches!(
            result,
            Err(GetProductError::ProductDetailsReadModelInvalid)
        ));
        let state = lock_state(&state);
        assert_eq!(1, state.commit_count);
        assert!(state.notification_requests.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_redact_hidden_search_filter_match_after_notification_hydration()
    -> Result<(), url::ParseError> {
        let state = state();
        let user_id = UserId::new();
        let event_id = EventId::new();
        let mut view = personalized_details_view()?;
        let original_lifecycle = view.item.lifecycle;
        if let Some(user_state) = view.user_state.as_mut() {
            user_state.search_filter.hidden = true;
        }
        lock_state(&state).find_details_result = Some(Ok(Some(view)));
        lock_state(&state).notification_result =
            Some(Ok(vec![notification_item(user_id, event_id, false)]));

        let result = handler(&state)
            .execute(&context(Principal::User(user_id)), request(Language::En))
            .await;

        match result {
            Ok(view) => {
                assert_eq!(
                    "00000000-0000-0000-0000-000000000000",
                    view.item.product_id.to_string()
                );
                assert!(view.item.product_slug_id.as_ref().starts_with("hidden-"));
                assert_eq!("hidden", view.item.shop_slug_id.as_ref());
                assert_eq!("hidden", view.item.seller_slug_id.as_ref());
                assert_eq!(ProductState::Unknown, view.item.state);
                assert_eq!(original_lifecycle, view.item.lifecycle);
                assert!(view.item.address.structured.is_none());
                assert!(view.item.address.geo.is_none());
                assert!(view.item.product_title.is_none());
                assert!(view.item.product_description.is_none());
                assert_eq!(
                    Some("Hidden Product Title"),
                    view.item.title.as_ref().map(|title| title.payload.as_ref())
                );
                assert!(view.item.description.is_none());
                assert_eq!(ProductPricing::default(), view.item.pricing);
                assert!(view.item.price.is_none());
                assert!(view.item.currency.is_none());
                assert_eq!("https://aura-historia.com/pricing", view.item.url.as_str());
                assert_eq!(
                    "https://aura-historia.com/pricing",
                    view.item.view_url.as_str()
                );
                assert!(view.item.images.is_empty());
                assert_eq!(ProductAuction::default(), view.item.auction);
                assert_eq!(OffsetDateTime::UNIX_EPOCH, view.item.created);
                assert_eq!(OffsetDateTime::UNIX_EPOCH, view.item.updated);
                let user_state = view.user_state.unwrap_or_default();
                assert!(user_state.search_filter.hidden);
                assert!(!user_state.notification.seen);
                assert_eq!(Some(event_id), user_state.notification.origin_event_id);
            }
            Err(error) => panic!("expected redacted details: {error}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_return_not_found_without_commit_or_notification_hydration() {
        let state = state();

        let result = handler(&state)
            .execute(&context(Principal::Anonymous), request(Language::En))
            .await;

        assert!(matches!(result, Err(GetProductError::NotFound)));
        let state = lock_state(&state);
        assert_eq!(0, state.commit_count);
        assert!(state.notification_requests.is_empty());
    }

    #[tokio::test]
    async fn should_map_detail_read_failure_without_commit_or_notification_hydration() {
        let state = state();
        lock_state(&state).find_details_result =
            Some(Err(ProductDetailsReadError::ProductDetailsQueryFailed));

        let result = handler(&state)
            .execute(&context(Principal::Anonymous), request(Language::En))
            .await;

        assert!(matches!(
            result,
            Err(GetProductError::ProductDetailsQueryFailed)
        ));
        let state = lock_state(&state);
        assert_eq!(0, state.commit_count);
        assert!(state.notification_requests.is_empty());
    }

    #[tokio::test]
    async fn should_map_begin_failure_without_read_or_notification_hydration() {
        let state = state();
        lock_state(&state).begin_error = true;

        let result = handler(&state)
            .execute(&context(Principal::Anonymous), request(Language::En))
            .await;

        assert!(matches!(
            result,
            Err(GetProductError::BeginTransactionFailed)
        ));
        let state = lock_state(&state);
        assert!(state.find_details_request.is_none());
        assert!(state.notification_requests.is_empty());
    }

    #[tokio::test]
    async fn should_map_commit_failure_without_notification_hydration()
    -> Result<(), url::ParseError> {
        let state = state();
        lock_state(&state).commit_error = true;
        lock_state(&state).find_details_result = Some(Ok(Some(personalized_details_view()?)));

        let result = handler(&state)
            .execute(
                &context(Principal::User(UserId::new())),
                request(Language::En),
            )
            .await;

        assert!(matches!(
            result,
            Err(GetProductError::CommitTransactionFailed)
        ));
        let state = lock_state(&state);
        assert_eq!(1, state.commit_count);
        assert!(state.notification_requests.is_empty());
        Ok(())
    }

    #[test]
    fn should_map_all_get_product_reader_errors() {
        assert!(matches!(
            GetProductError::from(ProductDetailsReadError::ProductDetailsReadModelInvalid),
            GetProductError::ProductDetailsReadModelInvalid
        ));
    }

    #[test]
    fn should_only_personalize_user_principals() {
        let user_id = UserId::new();

        assert_eq!(None, personalization_user_id(&Principal::Anonymous));
        assert_eq!(
            Some(user_id),
            personalization_user_id(&Principal::User(user_id))
        );
        assert_eq!(
            Some(user_id),
            personalization_user_id(&Principal::DelegatedUser {
                user_id,
                capabilities: Default::default(),
            })
        );
        assert_eq!(
            None,
            personalization_user_id(&Principal::Service("service".to_owned()))
        );
        assert_eq!(None, personalization_user_id(&Principal::System));
    }
}
