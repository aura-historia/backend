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
        let user_ids = self
            .watchlist_service
            .find_user_ids_with_notifications(&event.aggregate_id)
            .await?;
        if user_ids.is_empty() {
            return Ok(vec![]);
        }

        let product = self
            .get_product_service
            .find_product(event.payload.shop_id(), event.payload.shops_product_id())
            .await?;

        let notifications = user_ids
            .into_iter()
            .map(|user_id| CreateNotificationCommand {
                user_id,
                notification_payload: mk_notification_payload(&product, &event.payload),
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
        ProductDomainEventPayload::StateListed(payload) => {
            mk_state_change_watchlist_notification_payload(product, payload, &ProductState::Listed)
        }
        ProductDomainEventPayload::StateAvailable(payload) => {
            mk_state_change_watchlist_notification_payload(
                product,
                payload,
                &ProductState::Available,
            )
        }
        ProductDomainEventPayload::StateReserved(payload) => {
            mk_state_change_watchlist_notification_payload(
                product,
                payload,
                &ProductState::Reserved,
            )
        }
        ProductDomainEventPayload::StateSold(payload) => {
            mk_state_change_watchlist_notification_payload(product, payload, &ProductState::Sold)
        }
        ProductDomainEventPayload::StateRemoved(payload) => {
            mk_state_change_watchlist_notification_payload(product, payload, &ProductState::Removed)
        }
        ProductDomainEventPayload::StateUnknown(payload) => {
            mk_state_change_watchlist_notification_payload(product, payload, &ProductState::Unknown)
        }
        ProductDomainEventPayload::PriceDiscovered(payload) => {
            mk_price_change_watchlist_notification_payload(
                product,
                Default::default(),
                payload.prices(),
            )
        }
        ProductDomainEventPayload::PriceDropped(payload) => {
            mk_price_change_watchlist_notification_payload(
                product,
                payload.old_prices(),
                payload.new_prices(),
            )
        }
        ProductDomainEventPayload::PriceIncreased(payload) => {
            mk_price_change_watchlist_notification_payload(
                product,
                payload.old_prices(),
                payload.new_prices(),
            )
        }
        ProductDomainEventPayload::PriceRemoved(payload) => {
            mk_price_change_watchlist_notification_payload(
                product,
                payload.old_prices(),
                Default::default(),
            )
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
        event::Event, event_id::EventId, product_id::ProductId,
        product_state::domain::ProductState, user_id::UserId,
    };
    use fake::{Fake, Faker};
    use product::core::product::Product;
    use product::core::product_event::domain::{
        ProductCreatedDomainEventPayload, ProductDomainEventPayload,
        ProductPriceChangeDomainEventPayload, ProductPriceDiscoveryDomainEventPayload,
        ProductPriceRemovedDomainEventPayload, ProductStateChangeDomainEventPayload,
    };
    use product::service::get_service::{GetProductError, MockGetProductService};
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

    fn make_state_change_payload(old_state: ProductState) -> ProductStateChangeDomainEventPayload {
        let base: ProductStateChangeDomainEventPayload = Faker.fake();
        ProductStateChangeDomainEventPayload { old_state, ..base }
    }

    fn extract_watchlist_payload(cmd: &CreateNotificationCommand) -> &NotificationWatchlistPayload {
        match &cmd.notification_payload {
            NotificationPayload::Watchlist {
                watchlist_payload, ..
            } => watchlist_payload,
        }
    }

    fn extract_notification_fields(
        cmd: &CreateNotificationCommand,
    ) -> (
        ProductId,
        common::shop_id::ShopId,
        common::shops_product_id::ShopsProductId,
        common::slug_id::SlugId<0>,
        common::slug_id::SlugId<6>,
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
        }
    }

    // ---------------------------------------------------------------------------
    // Happy path – no watchers
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn should_return_empty_when_no_users_watching_product() {
        let mut watchlist_mock = MockProductWatchListService::new();
        watchlist_mock
            .expect_find_user_ids_with_notifications()
            .return_once(|_| Box::pin(async { Ok(vec![]) }));

        let get_product_mock = MockGetProductService::new();
        // get_product_service must NOT be called when there are no watchers
        // (MockGetProductService will panic on unexpected calls by default)

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(ProductDomainEventPayload::StateListed(Faker.fake()));

        let result = svc.determine_notification_commands(event).await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    // ---------------------------------------------------------------------------
    // Happy path – multiple watchers
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn should_create_notification_commands_for_all_watching_users() {
        let user_ids = vec![UserId::new(), UserId::new(), UserId::new()];
        let expected_user_ids = user_ids.clone();

        let mut watchlist_mock = MockProductWatchListService::new();
        watchlist_mock
            .expect_find_user_ids_with_notifications()
            .return_once(move |_| Box::pin(async move { Ok(user_ids) }));

        let mut get_product_mock = MockGetProductService::new();
        get_product_mock
            .expect_find_product()
            .return_once(|_, _| Box::pin(async { Ok(Faker.fake::<Product>()) }));

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(ProductDomainEventPayload::StateListed(Faker.fake()));

        let result = svc
            .determine_notification_commands(event)
            .await
            .expect("expected Ok");

        assert_eq!(3, result.len());

        let actual_user_ids: Vec<UserId> = result.iter().map(|cmd| cmd.user_id).collect();
        for user_id in &expected_user_ids {
            assert!(
                actual_user_ids.contains(user_id),
                "expected user_id {user_id} to be present in commands"
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
            .expect_find_user_ids_with_notifications()
            .return_once(|_| Box::pin(async { Ok(vec![UserId::new()]) }));

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
    #[case::listed(
        ProductState::Available,
        ProductDomainEventPayload::StateListed as fn(ProductStateChangeDomainEventPayload) -> ProductDomainEventPayload,
        ProductState::Listed,
    )]
    #[case::available(
        ProductState::Listed,
        ProductDomainEventPayload::StateAvailable as fn(ProductStateChangeDomainEventPayload) -> ProductDomainEventPayload,
        ProductState::Available,
    )]
    #[case::reserved(
        ProductState::Listed,
        ProductDomainEventPayload::StateReserved as fn(ProductStateChangeDomainEventPayload) -> ProductDomainEventPayload,
        ProductState::Reserved,
    )]
    #[case::sold(
        ProductState::Reserved,
        ProductDomainEventPayload::StateSold as fn(ProductStateChangeDomainEventPayload) -> ProductDomainEventPayload,
        ProductState::Sold,
    )]
    #[case::removed(
        ProductState::Listed,
        ProductDomainEventPayload::StateRemoved as fn(ProductStateChangeDomainEventPayload) -> ProductDomainEventPayload,
        ProductState::Removed,
    )]
    #[case::unknown(
        ProductState::Listed,
        ProductDomainEventPayload::StateUnknown as fn(ProductStateChangeDomainEventPayload) -> ProductDomainEventPayload,
        ProductState::Unknown,
    )]
    #[tokio::test]
    async fn should_map_state_event_to_state_change_notification_when_state_variant(
        #[case] old_state: ProductState,
        #[case] variant_ctor: fn(ProductStateChangeDomainEventPayload) -> ProductDomainEventPayload,
        #[case] expected_new_state: ProductState,
    ) {
        let state_payload = make_state_change_payload(old_state);

        let mut watchlist_mock = MockProductWatchListService::new();
        watchlist_mock
            .expect_find_user_ids_with_notifications()
            .return_once(|_| Box::pin(async { Ok(vec![UserId::new()]) }));

        let mut get_product_mock = MockGetProductService::new();
        get_product_mock
            .expect_find_product()
            .return_once(|_, _| Box::pin(async { Ok(Faker.fake::<Product>()) }));

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(variant_ctor(state_payload));

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
                } if *actual_old == old_state && *actual_new == expected_new_state
            ),
            "expected StateChange {{ old_state: {:?}, new_state: {:?} }}, got {:?}",
            old_state,
            expected_new_state,
            extract_watchlist_payload(&cmd)
        );
    }

    // ---------------------------------------------------------------------------
    // Payload mapping – PriceDiscovered → PriceChange with empty old_price
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn should_map_price_discovered_to_price_change_notification_with_empty_old_price() {
        let price_payload: ProductPriceDiscoveryDomainEventPayload = Faker.fake();
        let expected_new_prices = price_payload.prices();

        let mut watchlist_mock = MockProductWatchListService::new();
        watchlist_mock
            .expect_find_user_ids_with_notifications()
            .return_once(|_| Box::pin(async { Ok(vec![UserId::new()]) }));

        let mut get_product_mock = MockGetProductService::new();
        get_product_mock
            .expect_find_product()
            .return_once(|_, _| Box::pin(async { Ok(Faker.fake::<Product>()) }));

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(ProductDomainEventPayload::PriceDiscovered(price_payload));

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
            .expect_find_user_ids_with_notifications()
            .return_once(|_| Box::pin(async { Ok(vec![UserId::new()]) }));

        let mut get_product_mock = MockGetProductService::new();
        get_product_mock
            .expect_find_product()
            .return_once(|_, _| Box::pin(async { Ok(Faker.fake::<Product>()) }));

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(ProductDomainEventPayload::PriceDropped(price_payload));

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
            .expect_find_user_ids_with_notifications()
            .return_once(|_| Box::pin(async { Ok(vec![UserId::new()]) }));

        let mut get_product_mock = MockGetProductService::new();
        get_product_mock
            .expect_find_product()
            .return_once(|_, _| Box::pin(async { Ok(Faker.fake::<Product>()) }));

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(ProductDomainEventPayload::PriceIncreased(price_payload));

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
        let price_payload: ProductPriceRemovedDomainEventPayload = Faker.fake();
        let expected_old_prices = price_payload.old_prices();

        let mut watchlist_mock = MockProductWatchListService::new();
        watchlist_mock
            .expect_find_user_ids_with_notifications()
            .return_once(|_| Box::pin(async { Ok(vec![UserId::new()]) }));

        let mut get_product_mock = MockGetProductService::new();
        get_product_mock
            .expect_find_product()
            .return_once(|_, _| Box::pin(async { Ok(Faker.fake::<Product>()) }));

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(ProductDomainEventPayload::PriceRemoved(price_payload));

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
            .expect_find_user_ids_with_notifications()
            .return_once(|_| Box::pin(async { Ok(vec![UserId::new()]) }));

        let mut get_product_mock = MockGetProductService::new();
        get_product_mock
            .expect_find_product()
            .return_once(move |_, _| Box::pin(async move { Ok(product) }));

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(ProductDomainEventPayload::StateListed(Faker.fake()));

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
        };
        assert_eq!(expected_titles, actual_titles, "titles mismatch");
    }

    // ---------------------------------------------------------------------------
    // Unhappy path – watchlist service error propagates
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn should_propagate_watch_product_error_when_find_user_ids_fails() {
        let mut watchlist_mock = MockProductWatchListService::new();
        watchlist_mock
            .expect_find_user_ids_with_notifications()
            .return_once(|_| {
                Box::pin(async {
                    Err(WatchProductError::SdkGetItemError(
                        SdkError::construction_failure("simulated DynamoDB failure"),
                    ))
                })
            });

        let get_product_mock = MockGetProductService::new();

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(ProductDomainEventPayload::StateListed(Faker.fake()));

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
            .expect_find_user_ids_with_notifications()
            .return_once(|_| Box::pin(async { Ok(vec![UserId::new()]) }));

        let mut get_product_mock = MockGetProductService::new();
        get_product_mock.expect_find_product().return_once(|_, _| {
            Box::pin(async {
                Err(GetProductError::SdkGetItemError(
                    SdkError::construction_failure("simulated DynamoDB failure"),
                ))
            })
        });

        let svc =
            ProductEventWatchlistNotificationsServiceImpl::new(&watchlist_mock, &get_product_mock);

        let event = make_event(ProductDomainEventPayload::StateListed(Faker.fake()));

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
