use common::currency::domain::Currency;
use common::price::domain::MonetaryAmount;
use common::product_state::domain::ProductState;
use notification::core::notification::{NotificationPayload, NotificationWatchlistPayload};
use notification::service::command::CreateNotificationCommand;
use product::core::product::Product;
use product::core::product_event::ProductDomainEvent;
use product::core::product_event::domain::{
    ProductCommonEventPayload, ProductCreatedDomainEventPayload, ProductDomainEventPayload,
    ProductStateChangeDomainEventPayload,
};
use product::service::get_service::{GetProductError, GetProductService};
use product_watchlist::service::product_watchlist_service::{
    ProductWatchListService, WatchProductError,
};
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum ProductEventWatchlistNotificationsServiceError {
    #[error("WatchProductError: {0}")]
    WatchProductError(#[from] WatchProductError),

    #[error("GetProductError: {0}")]
    GetProductError(#[from] GetProductError),
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait ProductEventWatchlistNotificationsService {
    async fn determine_notification_commands(
        &self,
        event: ProductDomainEvent,
    ) -> Result<Vec<CreateNotificationCommand>, ProductEventWatchlistNotificationsServiceError>;
}

pub struct ProductEventWatchlistNotificationsServiceImpl<'a> {
    watchlist_service: &'a (dyn ProductWatchListService + Sync),
    get_product_service: &'a (dyn GetProductService + Sync),
}

impl<'a> ProductEventWatchlistNotificationsServiceImpl<'a> {
    pub fn new(
        watchlist_service: &'a (dyn ProductWatchListService + Sync),
        get_product_service: &'a (dyn GetProductService + Sync),
    ) -> Self {
        ProductEventWatchlistNotificationsServiceImpl {
            watchlist_service,
            get_product_service,
        }
    }
}

#[async_trait::async_trait]
impl<'a> ProductEventWatchlistNotificationsService
    for ProductEventWatchlistNotificationsServiceImpl<'a>
{
    async fn determine_notification_commands(
        &self,
        event: ProductDomainEvent,
    ) -> Result<Vec<CreateNotificationCommand>, ProductEventWatchlistNotificationsServiceError>
    {
        let watchlist_products = self
            .watchlist_service
            .find_user_ids_watching_product(&event.aggregate_id)
            .await?;
        if watchlist_products.is_empty() {
            return Ok(vec![]);
        }

        let product = self
            .get_product_service
            .find_product(event.payload.shop_id(), event.payload.shops_product_id())
            .await?;

        let notifications = watchlist_products
            .into_iter()
            .filter(|watchlist_product| watchlist_product.state.is_active())
            .map(|watchlist_product| CreateNotificationCommand {
                user_id: watchlist_product.user_id,
                notification_payload: mk_notification_payload(&product, &event.payload),
                external: watchlist_product.notifications,
            })
            .collect();
        Ok(notifications)
    }
}

fn mk_notification_payload(
    product: &Product,
    event_payload: &ProductDomainEventPayload,
) -> NotificationPayload {
    match event_payload {
        ProductDomainEventPayload::Created(payload) => {
            mk_created_watchlist_notification_payload(product, payload)
        }
        ProductDomainEventPayload::StateChanged(payload) => {
            mk_state_change_watchlist_notification_payload(product, payload, &payload.new_state)
        }
        ProductDomainEventPayload::PriceChanged(payload) => {
            mk_price_change_watchlist_notification_payload(
                product,
                payload.old_prices(),
                payload.new_prices(),
            )
        }
        ProductDomainEventPayload::EstimatePriceChanged(_)
        | ProductDomainEventPayload::UrlChanged(_)
        | ProductDomainEventPayload::ImagesChanged(_)
        | ProductDomainEventPayload::AuctionTimeChanged(_) => {
            unreachable!("Field-level change events are not routed to the notification handler")
        }
    }
}

fn mk_created_watchlist_notification_payload(
    product: &Product,
    payload: &ProductCreatedDomainEventPayload,
) -> NotificationPayload {
    NotificationPayload::Watchlist {
        product_id: product.product_id,
        shop_id: product.shop_id,
        shops_product_id: product.shops_product_id.clone(),
        shop_slug_id: product.shop_slug_id.clone(),
        product_slug_id: product.product_slug_id.clone(),
        shop_name: product.shop_name.clone(),
        title: product.titles(),
        image: product.images.first().cloned(),
        url: product.url.clone(),
        view_url: product.view_url.clone(),
        watchlist_payload: NotificationWatchlistPayload::StateChange {
            old_state: ProductState::Unknown,
            new_state: payload.state,
        },
    }
}

fn mk_state_change_watchlist_notification_payload(
    product: &Product,
    payload: &ProductStateChangeDomainEventPayload,
    new_state: &ProductState,
) -> NotificationPayload {
    NotificationPayload::Watchlist {
        product_id: product.product_id,
        shop_id: product.shop_id,
        shops_product_id: product.shops_product_id.clone(),
        shop_slug_id: product.shop_slug_id.clone(),
        product_slug_id: product.product_slug_id.clone(),
        shop_name: product.shop_name.clone(),
        title: product.titles(),
        image: product.images.first().cloned(),
        url: product.url.clone(),
        view_url: product.view_url.clone(),
        watchlist_payload: NotificationWatchlistPayload::StateChange {
            old_state: payload.old_state,
            new_state: *new_state,
        },
    }
}

fn mk_price_change_watchlist_notification_payload(
    product: &Product,
    old_price: HashMap<Currency, MonetaryAmount>,
    new_price: HashMap<Currency, MonetaryAmount>,
) -> NotificationPayload {
    NotificationPayload::Watchlist {
        product_id: product.product_id,
        shop_id: product.shop_id,
        shops_product_id: product.shops_product_id.clone(),
        shop_slug_id: product.shop_slug_id.clone(),
        product_slug_id: product.product_slug_id.clone(),
        shop_name: product.shop_name.clone(),
        title: product.titles(),
        image: product.images.first().cloned(),
        url: product.url.clone(),
        view_url: product.view_url.clone(),
        watchlist_payload: NotificationWatchlistPayload::PriceChange {
            old_price,
            new_price,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_dynamodb::error::SdkError;
    use common::{
        actor::domain::Actor, event::Event, event_id::EventId, product_id::ProductId,
        product_state::domain::ProductState, user_id::UserId,
    };
    use fake::{Fake, Faker};
    use product::core::product::Product;
    use product::core::product_event::domain::{
        ProductCreatedDomainEventPayload, ProductDomainEventPayload,
        ProductPriceChangeDomainEventPayload, ProductStateChangeDomainEventPayload,
    };
    use product::service::get_service::{GetProductError, MockGetProductService};
    use product_watchlist::core::watchlist_product::WatchlistProduct;
    use product_watchlist::service::product_watchlist_service::{
        MockProductWatchListService, WatchProductError,
    };
    use time::OffsetDateTime;

    // ---------------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------------

    fn make_event(payload: ProductDomainEventPayload) -> ProductDomainEvent {
        Event {
            aggregate_id: ProductId::new(),
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload,
        }
    }

    fn watchlist_product(user_id: UserId, notifications: bool) -> WatchlistProduct {
        WatchlistProduct {
            user_id,
            shop_id: Faker.fake(),
            shops_product_id: Faker.fake(),
            product_id: ProductId::new(),
            notifications,
            state: common::resource_state::domain::ResourceState::Active,
            created_by: Actor::User(user_id),
            updated_by: Actor::User(user_id),
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        }
    }

    fn extract_watchlist_payload(cmd: &CreateNotificationCommand) -> &NotificationWatchlistPayload {
        match &cmd.notification_payload {
            NotificationPayload::Watchlist {
                watchlist_payload, ..
            } => watchlist_payload,
            _ => unreachable!("expected Watchlist payload"),
        }
    }

    fn extract_notification_fields(
        cmd: &CreateNotificationCommand,
    ) -> (
        ProductId,
        common::shop_id::ShopId,
        common::shops_product_id::ShopsProductId,
        common::shop_slug_id::ShopSlugId,
        common::product_slug_id::ProductSlugId,
        common::shop_name::ShopName,
    ) {
        match &cmd.notification_payload {
            NotificationPayload::Watchlist {
                product_id,
                shop_id,
                shops_product_id,
                shop_slug_id,
                product_slug_id,
                shop_name,
                ..
            } => (
                *product_id,
                *shop_id,
                shops_product_id.clone(),
                shop_slug_id.clone(),
                product_slug_id.clone(),
                shop_name.clone(),
            ),
            _ => unreachable!("expected Watchlist payload"),
        }
    }

    // ---------------------------------------------------------------------------
    // Happy path – no watchers
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn should_return_empty_when_no_users_watching_product() {
        let mut watchlist_mock = MockProductWatchListService::new();
        watchlist_mock
            .expect_find_user_ids_watching_product()
            .return_once(|_| Box::pin(async { Ok(vec![]) }));

        let get_product_mock = MockGetProductService::new();
        // get_product_service must NOT be called when there are no watchers
        // (MockGetProductService will panic on unexpected calls by default)

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(ProductDomainEventPayload::StateChanged(Faker.fake()));

        let result = svc.determine_notification_commands(event).await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    // ---------------------------------------------------------------------------
    // Happy path – multiple watchers
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn should_create_notification_commands_for_all_watching_users() {
        // (user_id, notifications_enabled)
        let watching = vec![
            watchlist_product(UserId::new(), true),
            watchlist_product(UserId::new(), false),
            watchlist_product(UserId::new(), true),
        ];
        let expected: Vec<(UserId, bool)> = watching
            .iter()
            .map(|w| (w.user_id, w.notifications))
            .collect();

        let mut watchlist_mock = MockProductWatchListService::new();
        watchlist_mock
            .expect_find_user_ids_watching_product()
            .return_once(move |_| Box::pin(async move { Ok(watching) }));

        let mut get_product_mock = MockGetProductService::new();
        get_product_mock
            .expect_find_product()
            .return_once(|_, _| Box::pin(async { Ok(Faker.fake::<Product>()) }));

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(ProductDomainEventPayload::StateChanged(Faker.fake()));

        let result = svc
            .determine_notification_commands(event)
            .await
            .expect("expected Ok");

        assert_eq!(3, result.len());

        for (expected_user_id, expected_external) in &expected {
            let cmd = result
                .iter()
                .find(|cmd| &cmd.user_id == expected_user_id)
                .unwrap_or_else(|| {
                    panic!("expected user_id {expected_user_id} to be present in commands")
                });
            assert_eq!(
                cmd.external, *expected_external,
                "expected external={expected_external} for user_id {expected_user_id}"
            );
        }
    }

    // ---------------------------------------------------------------------------
    // Payload mapping – Created → StateChange with Unknown old_state
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn should_map_created_event_to_state_change_notification_with_unknown_old_state() {
        let created_payload: ProductCreatedDomainEventPayload = Faker.fake();
        let expected_new_state = created_payload.state;

        let mut watchlist_mock = MockProductWatchListService::new();
        watchlist_mock
            .expect_find_user_ids_watching_product()
            .return_once(|_| Box::pin(async { Ok(vec![watchlist_product(UserId::new(), true)]) }));

        let mut get_product_mock = MockGetProductService::new();
        get_product_mock
            .expect_find_product()
            .return_once(|_, _| Box::pin(async { Ok(Faker.fake::<Product>()) }));

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(ProductDomainEventPayload::Created(created_payload));

        let mut result = svc
            .determine_notification_commands(event)
            .await
            .expect("expected Ok");

        assert_eq!(1, result.len());
        let cmd = result.remove(0);

        assert!(
            matches!(
                extract_watchlist_payload(&cmd),
                NotificationWatchlistPayload::StateChange {
                    old_state: ProductState::Unknown,
                    new_state,
                } if *new_state == expected_new_state
            ),
            "expected StateChange {{ old_state: Unknown, new_state: {:?} }}, got {:?}",
            expected_new_state,
            extract_watchlist_payload(&cmd)
        );
    }

    // ---------------------------------------------------------------------------
    // Payload mapping – state-change variants (parameterised with rstest)
    // ---------------------------------------------------------------------------

    #[rstest::rstest]
    #[case::listed(ProductState::Available, ProductState::Listed)]
    #[case::available(ProductState::Listed, ProductState::Available)]
    #[case::reserved(ProductState::Listed, ProductState::Reserved)]
    #[case::sold(ProductState::Reserved, ProductState::Sold)]
    #[case::removed(ProductState::Listed, ProductState::Removed)]
    #[case::unknown(ProductState::Listed, ProductState::Unknown)]
    #[tokio::test]
    async fn should_map_state_event_to_state_change_notification_when_state_variant(
        #[case] old_state: ProductState,
        #[case] new_state: ProductState,
    ) {
        let base: ProductStateChangeDomainEventPayload = Faker.fake();
        let state_payload = ProductStateChangeDomainEventPayload {
            old_state,
            new_state,
            ..base
        };

        let mut watchlist_mock = MockProductWatchListService::new();
        watchlist_mock
            .expect_find_user_ids_watching_product()
            .return_once(|_| Box::pin(async { Ok(vec![watchlist_product(UserId::new(), true)]) }));

        let mut get_product_mock = MockGetProductService::new();
        get_product_mock
            .expect_find_product()
            .return_once(|_, _| Box::pin(async { Ok(Faker.fake::<Product>()) }));

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(ProductDomainEventPayload::StateChanged(state_payload));

        let mut result = svc
            .determine_notification_commands(event)
            .await
            .expect("expected Ok");

        assert_eq!(1, result.len());
        let cmd = result.remove(0);

        assert!(
            matches!(
                extract_watchlist_payload(&cmd),
                NotificationWatchlistPayload::StateChange {
                    old_state: actual_old,
                    new_state: actual_new,
                } if *actual_old == old_state && *actual_new == new_state
            ),
            "expected StateChange {{ old_state: {:?}, new_state: {:?} }}, got {:?}",
            old_state,
            new_state,
            extract_watchlist_payload(&cmd)
        );
    }

    // ---------------------------------------------------------------------------
    // Payload mapping – PriceDiscovered → PriceChange with empty old_price
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn should_map_price_discovered_to_price_change_notification_with_empty_old_price() {
        let base: ProductPriceChangeDomainEventPayload = Faker.fake();
        let price_payload = ProductPriceChangeDomainEventPayload {
            old_native_price: None,
            old_other_price: HashMap::new(),
            ..base
        };
        let expected_new_prices = price_payload.new_prices();

        let mut watchlist_mock = MockProductWatchListService::new();
        watchlist_mock
            .expect_find_user_ids_watching_product()
            .return_once(|_| Box::pin(async { Ok(vec![watchlist_product(UserId::new(), true)]) }));

        let mut get_product_mock = MockGetProductService::new();
        get_product_mock
            .expect_find_product()
            .return_once(|_, _| Box::pin(async { Ok(Faker.fake::<Product>()) }));

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(ProductDomainEventPayload::PriceChanged(price_payload));

        let mut result = svc
            .determine_notification_commands(event)
            .await
            .expect("expected Ok");

        assert_eq!(1, result.len());
        let cmd = result.remove(0);

        assert!(
            matches!(
                extract_watchlist_payload(&cmd),
                NotificationWatchlistPayload::PriceChange { old_price, new_price }
                    if old_price.is_empty() && *new_price == expected_new_prices
            ),
            "expected PriceChange with empty old_price and non-empty new_price, got {:?}",
            extract_watchlist_payload(&cmd)
        );
    }

    // ---------------------------------------------------------------------------
    // Payload mapping – PriceDropped → PriceChange
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn should_map_price_dropped_to_price_change_notification_when_price_dropped() {
        let price_payload: ProductPriceChangeDomainEventPayload = Faker.fake();
        let expected_old_prices = price_payload.old_prices();
        let expected_new_prices = price_payload.new_prices();

        let mut watchlist_mock = MockProductWatchListService::new();
        watchlist_mock
            .expect_find_user_ids_watching_product()
            .return_once(|_| Box::pin(async { Ok(vec![watchlist_product(UserId::new(), true)]) }));

        let mut get_product_mock = MockGetProductService::new();
        get_product_mock
            .expect_find_product()
            .return_once(|_, _| Box::pin(async { Ok(Faker.fake::<Product>()) }));

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(ProductDomainEventPayload::PriceChanged(price_payload));

        let mut result = svc
            .determine_notification_commands(event)
            .await
            .expect("expected Ok");

        assert_eq!(1, result.len());
        let cmd = result.remove(0);

        assert!(
            matches!(
                extract_watchlist_payload(&cmd),
                NotificationWatchlistPayload::PriceChange { old_price, new_price }
                    if *old_price == expected_old_prices && *new_price == expected_new_prices
            ),
            "expected PriceChange with matching old/new prices, got {:?}",
            extract_watchlist_payload(&cmd)
        );
    }

    // ---------------------------------------------------------------------------
    // Payload mapping – PriceIncreased → PriceChange
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn should_map_price_increased_to_price_change_notification_when_price_increased() {
        let price_payload: ProductPriceChangeDomainEventPayload = Faker.fake();
        let expected_old_prices = price_payload.old_prices();
        let expected_new_prices = price_payload.new_prices();

        let mut watchlist_mock = MockProductWatchListService::new();
        watchlist_mock
            .expect_find_user_ids_watching_product()
            .return_once(|_| Box::pin(async { Ok(vec![watchlist_product(UserId::new(), true)]) }));

        let mut get_product_mock = MockGetProductService::new();
        get_product_mock
            .expect_find_product()
            .return_once(|_, _| Box::pin(async { Ok(Faker.fake::<Product>()) }));

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(ProductDomainEventPayload::PriceChanged(price_payload));

        let mut result = svc
            .determine_notification_commands(event)
            .await
            .expect("expected Ok");

        assert_eq!(1, result.len());
        let cmd = result.remove(0);

        assert!(
            matches!(
                extract_watchlist_payload(&cmd),
                NotificationWatchlistPayload::PriceChange { old_price, new_price }
                    if *old_price == expected_old_prices && *new_price == expected_new_prices
            ),
            "expected PriceChange with matching old/new prices, got {:?}",
            extract_watchlist_payload(&cmd)
        );
    }

    // ---------------------------------------------------------------------------
    // Payload mapping – PriceRemoved → PriceChange with empty new_price
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn should_map_price_removed_to_price_change_notification_with_empty_new_price() {
        let base: ProductPriceChangeDomainEventPayload = Faker.fake();
        let price_payload = ProductPriceChangeDomainEventPayload {
            new_native_price: None,
            new_other_price: HashMap::new(),
            ..base
        };
        let expected_old_prices = price_payload.old_prices();

        let mut watchlist_mock = MockProductWatchListService::new();
        watchlist_mock
            .expect_find_user_ids_watching_product()
            .return_once(|_| Box::pin(async { Ok(vec![watchlist_product(UserId::new(), true)]) }));

        let mut get_product_mock = MockGetProductService::new();
        get_product_mock
            .expect_find_product()
            .return_once(|_, _| Box::pin(async { Ok(Faker.fake::<Product>()) }));

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(ProductDomainEventPayload::PriceChanged(price_payload));

        let mut result = svc
            .determine_notification_commands(event)
            .await
            .expect("expected Ok");

        assert_eq!(1, result.len());
        let cmd = result.remove(0);

        assert!(
            matches!(
                extract_watchlist_payload(&cmd),
                NotificationWatchlistPayload::PriceChange { old_price, new_price }
                    if *old_price == expected_old_prices && new_price.is_empty()
            ),
            "expected PriceChange with non-empty old_price and empty new_price, got {:?}",
            extract_watchlist_payload(&cmd)
        );
    }

    // ---------------------------------------------------------------------------
    // Payload mapping – product fields are forwarded correctly
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn should_map_product_fields_correctly_to_notification_when_building_command() {
        let product: Product = Faker.fake();

        let expected_product_id = product.product_id;
        let expected_shop_id = product.shop_id;
        let expected_shops_product_id = product.shops_product_id.clone();
        let expected_shop_slug_id = product.shop_slug_id.clone();
        let expected_product_slug_id = product.product_slug_id.clone();
        let expected_shop_name = product.shop_name.clone();
        let expected_titles = product.titles();

        let mut watchlist_mock = MockProductWatchListService::new();
        watchlist_mock
            .expect_find_user_ids_watching_product()
            .return_once(|_| Box::pin(async { Ok(vec![watchlist_product(UserId::new(), true)]) }));

        let mut get_product_mock = MockGetProductService::new();
        get_product_mock
            .expect_find_product()
            .return_once(move |_, _| Box::pin(async move { Ok(product) }));

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(ProductDomainEventPayload::StateChanged(Faker.fake()));

        let mut result = svc
            .determine_notification_commands(event)
            .await
            .expect("expected Ok");

        assert_eq!(1, result.len());
        let cmd = result.remove(0);

        let (product_id, shop_id, shops_product_id, shop_slug_id, product_slug_id, shop_name) =
            extract_notification_fields(&cmd);

        assert_eq!(expected_product_id, product_id, "product_id mismatch");
        assert_eq!(expected_shop_id, shop_id, "shop_id mismatch");
        assert_eq!(
            expected_shops_product_id, shops_product_id,
            "shops_product_id mismatch"
        );
        assert_eq!(expected_shop_slug_id, shop_slug_id, "shop_slug_id mismatch");
        assert_eq!(
            expected_product_slug_id, product_slug_id,
            "product_slug_id mismatch"
        );
        assert_eq!(expected_shop_name, shop_name, "shop_name mismatch");

        let actual_titles = match &cmd.notification_payload {
            NotificationPayload::Watchlist { title, .. } => title.clone(),
            _ => unreachable!("expected Watchlist payload"),
        };
        assert_eq!(expected_titles, actual_titles, "titles mismatch");
    }

    // ---------------------------------------------------------------------------
    // Payload mapping – first image is forwarded to notification command
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn should_set_first_image_when_product_has_images() {
        let first_image: product::core::product_image::ProductImage = Faker.fake();
        let second_image: product::core::product_image::ProductImage = Faker.fake();
        let base: Product = Faker.fake();
        let product = Product {
            images: vec![first_image.clone(), second_image]
                .into_iter()
                .collect(),
            ..base
        };
        let expected_image = first_image;

        let mut watchlist_mock = MockProductWatchListService::new();
        watchlist_mock
            .expect_find_user_ids_watching_product()
            .return_once(|_| Box::pin(async { Ok(vec![watchlist_product(UserId::new(), true)]) }));

        let mut get_product_mock = MockGetProductService::new();
        get_product_mock
            .expect_find_product()
            .return_once(move |_, _| Box::pin(async move { Ok(product) }));

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(ProductDomainEventPayload::StateChanged(Faker.fake()));

        let mut result = svc
            .determine_notification_commands(event)
            .await
            .expect("expected Ok");

        assert_eq!(1, result.len());
        let cmd = result.remove(0);

        let actual_image = match &cmd.notification_payload {
            NotificationPayload::Watchlist { image, .. } => image.clone(),
            _ => unreachable!("expected Watchlist payload"),
        };
        assert_eq!(
            Some(expected_image),
            actual_image,
            "expected first image to be set"
        );
    }

    #[tokio::test]
    async fn should_set_image_to_none_when_product_has_no_images() {
        let base: Product = Faker.fake();
        let product = Product {
            images: Default::default(),
            ..base
        };

        let mut watchlist_mock = MockProductWatchListService::new();
        watchlist_mock
            .expect_find_user_ids_watching_product()
            .return_once(|_| Box::pin(async { Ok(vec![watchlist_product(UserId::new(), true)]) }));

        let mut get_product_mock = MockGetProductService::new();
        get_product_mock
            .expect_find_product()
            .return_once(move |_, _| Box::pin(async move { Ok(product) }));

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(ProductDomainEventPayload::StateChanged(Faker.fake()));

        let mut result = svc
            .determine_notification_commands(event)
            .await
            .expect("expected Ok");

        assert_eq!(1, result.len());
        let cmd = result.remove(0);

        let actual_image = match &cmd.notification_payload {
            NotificationPayload::Watchlist { image, .. } => image.clone(),
            _ => unreachable!("expected Watchlist payload"),
        };
        assert!(actual_image.is_none(), "expected image to be None");
    }

    // ---------------------------------------------------------------------------
    // Unhappy path – watchlist service error propagates
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn should_propagate_watch_product_error_when_find_user_ids_fails() {
        let mut watchlist_mock = MockProductWatchListService::new();
        watchlist_mock
            .expect_find_user_ids_watching_product()
            .return_once(|_| {
                Box::pin(async {
                    Err(WatchProductError::SdkGetItemError(Box::new(
                        SdkError::construction_failure("simulated DynamoDB failure"),
                    )))
                })
            });

        let get_product_mock = MockGetProductService::new();

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(ProductDomainEventPayload::StateChanged(Faker.fake()));

        let result = svc.determine_notification_commands(event).await;

        assert!(
            matches!(
                result,
                Err(ProductEventWatchlistNotificationsServiceError::WatchProductError(_))
            ),
            "expected WatchProductError variant, got {:?}",
            result
        );
    }

    // ---------------------------------------------------------------------------
    // Unhappy path – get-product service error propagates
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn should_propagate_get_product_error_when_find_product_fails() {
        let mut watchlist_mock = MockProductWatchListService::new();
        watchlist_mock
            .expect_find_user_ids_watching_product()
            .return_once(|_| Box::pin(async { Ok(vec![watchlist_product(UserId::new(), true)]) }));

        let mut get_product_mock = MockGetProductService::new();
        get_product_mock.expect_find_product().return_once(|_, _| {
            Box::pin(async {
                Err(GetProductError::SdkGetItemError(Box::new(
                    SdkError::construction_failure("simulated DynamoDB failure"),
                )))
            })
        });

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(ProductDomainEventPayload::StateChanged(Faker.fake()));

        let result = svc.determine_notification_commands(event).await;

        assert!(
            matches!(
                result,
                Err(ProductEventWatchlistNotificationsServiceError::GetProductError(_))
            ),
            "expected GetProductError variant, got {:?}",
            result
        );
    }
}
