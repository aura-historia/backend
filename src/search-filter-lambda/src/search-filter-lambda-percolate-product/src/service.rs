use common::has_key::HasKey;
use notification::core::notification::{NotificationPayload, NotificationSearchFilterPayload};
use notification::service::command::CreateNotificationCommand;
use product::core::product::Product;
use product::core::product_event::ProductEvent;
use product::core::product_event::domain::ProductDomainEventPayload;
use product::opensearch::product_document::ProductDocument;
use product::service::get_service::{GetProductError, GetProductService};
use search_filter::core::user_search_filter::UserSearchFilterSummary;
use search_filter::service::user_search_filter_service::{
    UserSearchFilterError, UserSearchFilterService,
};
use tracing::debug;

#[derive(Debug, thiserror::Error)]
pub enum ProductEventSearchFilterNotificationsServiceError {
    #[error("GetProductError: {0}")]
    GetProductError(#[from] GetProductError),

    #[error("UserSearchFilterError: {0}")]
    UserSearchFilterError(#[from] UserSearchFilterError),
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait ProductEventSearchFilterNotificationsService {
    async fn determine_notification_commands(
        &self,
        event: ProductEvent,
    ) -> Result<Vec<CreateNotificationCommand>, ProductEventSearchFilterNotificationsServiceError>;
}

pub struct ProductEventSearchFilterNotificationsServiceImpl<'a> {
    user_search_filter_service: &'a (dyn UserSearchFilterService + Sync),
    get_product_service: &'a (dyn GetProductService + Sync),
}

impl<'a> ProductEventSearchFilterNotificationsServiceImpl<'a> {
    pub fn new(
        user_search_filter_service: &'a (dyn UserSearchFilterService + Sync),
        get_product_service: &'a (dyn GetProductService + Sync),
    ) -> Self {
        Self {
            user_search_filter_service,
            get_product_service,
        }
    }
}

#[async_trait::async_trait]
impl<'a> ProductEventSearchFilterNotificationsService
    for ProductEventSearchFilterNotificationsServiceImpl<'a>
{
    async fn determine_notification_commands(
        &self,
        event: ProductEvent,
    ) -> Result<Vec<CreateNotificationCommand>, ProductEventSearchFilterNotificationsServiceError>
    {
        let product_key = event.payload.key();
        let product = match event.payload {
            product::core::product_event::ProductEventPayload::ProductDomainEvent(
                ProductDomainEventPayload::Created(created_payload),
            ) => Product {
                product_id: event.aggregate_id,
                product_slug_id: created_payload.product_slug_id,
                shop_slug_id: created_payload.shop_slug_id,
                event_id: event.event_id,
                shop_id: created_payload.shop_id,
                shops_product_id: created_payload.shops_product_id,
                shop_name: created_payload.shop_name,
                shop_type: created_payload.shop_type,
                category_id: None,
                category_name: Default::default(),
                period_id: None,
                period_name: Default::default(),
                native_title: created_payload.native_title,
                other_title: Default::default(),
                native_description: created_payload.native_description,
                other_description: Default::default(),
                native_price: created_payload.native_price,
                other_price: created_payload.other_price,
                native_price_estimate_min: created_payload.native_price_estimate_min,
                other_price_estimate_min: created_payload.other_price_estimate_min,
                native_price_estimate_max: created_payload.native_price_estimate_max,
                other_price_estimate_max: created_payload.other_price_estimate_max,
                state: created_payload.state,
                url: created_payload.url,
                images: created_payload.images,
                text_embedding: None,
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                auction_start: created_payload.auction_start,
                auction_end: created_payload.auction_end,
                created: event.timestamp,
                updated: event.timestamp,
            },
            _ => {
                let mut product = self
                    .get_product_service
                    .find_product(&product_key.shop_id, &product_key.shops_product_id)
                    .await?;
                product.apply(event);
                product
            }
        };

        let product_document = ProductDocument::from(product.clone());
        let matched_filters = self
            .user_search_filter_service
            .match_user_search_filters(&product_document)
            .await?;

        if matched_filters.is_empty() {
            return Ok(vec![]);
        }

        debug!(
            matched = matched_filters.len(),
            "Matched search filters for product."
        );

        // Filter out search filters that have already been matched for this product
        let mut unmatched_filters = Vec::with_capacity(matched_filters.len());
        for filter in matched_filters {
            let existing_match = self
                .user_search_filter_service
                .find_search_filter_product_match(
                    &filter.user_id,
                    &filter.user_search_filter_id,
                    &product.shop_id,
                    &product.shops_product_id,
                )
                .await?;
            if existing_match.is_none() {
                unmatched_filters.push(filter);
            } else {
                debug!(
                    userId = %filter.user_id,
                    searchFilterId = %filter.user_search_filter_id,
                    shopId = %product.shop_id,
                    shopsProductId = %product.shops_product_id,
                    "Skipping already-matched search filter for product."
                );
            }
        }

        if unmatched_filters.is_empty() {
            return Ok(vec![]);
        }

        let commands = unmatched_filters
            .into_iter()
            .map(|filter| mk_search_filter_notification_command(&product, filter))
            .collect();

        Ok(commands)
    }
}

fn mk_search_filter_notification_command(
    product: &Product,
    filter: UserSearchFilterSummary,
) -> CreateNotificationCommand {
    CreateNotificationCommand {
        user_id: filter.user_id,
        notification_payload: NotificationPayload::SearchFilter {
            product_id: product.product_id,
            shop_id: product.shop_id,
            shops_product_id: product.shops_product_id.clone(),
            shop_slug_id: product.shop_slug_id.clone(),
            product_slug_id: product.product_slug_id.clone(),
            shop_name: product.shop_name.clone(),
            title: product.titles(),
            search_filter_payload: NotificationSearchFilterPayload {
                user_search_filter_id: filter.user_search_filter_id,
                user_search_filter_name: filter.name,
            },
        },
        external: filter.notifications,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::event::Event;
    use common::event_id::EventId;
    use common::product_state::domain::ProductState;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use product::core::product_event::ProductEventPayload;
    use product::core::product_event::domain::{
        ProductCreatedDomainEventPayload, ProductDomainEventPayload,
        ProductStateChangeDomainEventPayload,
    };
    use product::service::get_service::MockGetProductService;
    use search_filter::core::user_search_filter::UserSearchFilterSummary;
    use search_filter::core::user_search_filter_id::UserSearchFilterId;
    use search_filter::core::user_search_filter_name::UserSearchFilterName;
    use search_filter::service::user_search_filter_service::MockUserSearchFilterService;
    use serde::de::Error as _;
    use time::OffsetDateTime;

    fn mk_event(product: &Product) -> ProductEvent {
        Event {
            aggregate_id: product.product_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductEventPayload::ProductDomainEvent(
                ProductDomainEventPayload::StateListed(ProductStateChangeDomainEventPayload {
                    shop_id: product.shop_id,
                    shops_product_id: product.shops_product_id.clone(),
                    old_state: ProductState::Available,
                }),
            ),
        }
    }

    fn mk_created_event(product: &Product) -> ProductEvent {
        Event {
            aggregate_id: product.product_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductEventPayload::ProductDomainEvent(ProductDomainEventPayload::Created(
                ProductCreatedDomainEventPayload {
                    product_slug_id: product.product_slug_id.clone(),
                    shop_slug_id: product.shop_slug_id.clone(),
                    shop_id: product.shop_id,
                    shops_product_id: product.shops_product_id.clone(),
                    shop_name: product.shop_name.clone(),
                    shop_type: product.shop_type,
                    native_title: product.native_title.clone(),
                    native_description: product.native_description.clone(),
                    native_price: product.native_price,
                    other_price: Default::default(),
                    native_price_estimate_min: product.native_price_estimate_min,
                    other_price_estimate_min: Default::default(),
                    native_price_estimate_max: product.native_price_estimate_max,
                    other_price_estimate_max: Default::default(),
                    state: product.state,
                    url: product.url.clone(),
                    images: product.images.clone(),
                    auction_start: product.auction_start,
                    auction_end: product.auction_end,
                },
            )),
        }
    }

    fn mk_filter_summary(user_id: UserId) -> UserSearchFilterSummary {
        UserSearchFilterSummary {
            user_id,
            user_search_filter_id: UserSearchFilterId::new(),
            name: UserSearchFilterName::from("Test Filter"),
            notifications: true,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        }
    }

    #[tokio::test]
    async fn should_return_empty_when_no_filters_match_for_determine_commands() {
        let product: Product = Faker.fake();
        let event = mk_event(&product);
        let product_clone = product.clone();

        let mut get_service = MockGetProductService::default();
        get_service
            .expect_find_product()
            .return_once(move |_, _| Box::pin(async move { Ok(product_clone) }));

        let mut filter_service = MockUserSearchFilterService::default();
        filter_service
            .expect_match_user_search_filters()
            .return_once(|_| Box::pin(async { Ok(vec![]) }));

        let service =
            ProductEventSearchFilterNotificationsServiceImpl::new(&filter_service, &get_service);

        let result = service.determine_notification_commands(event).await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn should_return_commands_when_filters_match_for_determine_commands() {
        let product: Product = Faker.fake();
        let event = mk_event(&product);
        let product_clone = product.clone();
        let user_id = UserId::new();
        let summary = mk_filter_summary(user_id);

        let mut get_service = MockGetProductService::default();
        get_service
            .expect_find_product()
            .return_once(move |_, _| Box::pin(async move { Ok(product_clone) }));

        let mut filter_service = MockUserSearchFilterService::default();
        filter_service
            .expect_match_user_search_filters()
            .return_once(move |_| Box::pin(async move { Ok(vec![summary]) }));
        filter_service
            .expect_find_search_filter_product_match()
            .return_once(|_, _, _, _| Box::pin(async { Ok(None) }));

        let service =
            ProductEventSearchFilterNotificationsServiceImpl::new(&filter_service, &get_service);

        let result = service.determine_notification_commands(event).await;

        assert!(result.is_ok());
        let cmds = result.unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].user_id, user_id);
        assert!(cmds[0].external);
    }

    #[tokio::test]
    async fn should_propagate_get_product_error_when_find_product_fails() {
        let product: Product = Faker.fake();
        let event = mk_event(&product);

        let mut get_service = MockGetProductService::default();
        get_service.expect_find_product().return_once(|_, _| {
            Box::pin(async { Err(GetProductError::ProductNotFound(Faker.fake(), Faker.fake())) })
        });

        let filter_service = MockUserSearchFilterService::default();

        let service =
            ProductEventSearchFilterNotificationsServiceImpl::new(&filter_service, &get_service);

        let result = service.determine_notification_commands(event).await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ProductEventSearchFilterNotificationsServiceError::GetProductError(_)
        ));
    }

    #[tokio::test]
    async fn should_propagate_search_filter_error_when_match_fails() {
        let product: Product = Faker.fake();
        let event = mk_event(&product);
        let product_clone = product.clone();

        let mut get_service = MockGetProductService::default();
        get_service
            .expect_find_product()
            .return_once(move |_, _| Box::pin(async move { Ok(product_clone) }));

        let mut filter_service = MockUserSearchFilterService::default();
        filter_service
            .expect_match_user_search_filters()
            .return_once(|_| {
                Box::pin(async {
                    Err(UserSearchFilterError::OpenSearchError(
                        opensearch::Error::from(serde_json::Error::custom("test error")),
                    ))
                })
            });

        let service =
            ProductEventSearchFilterNotificationsServiceImpl::new(&filter_service, &get_service);

        let result = service.determine_notification_commands(event).await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ProductEventSearchFilterNotificationsServiceError::UserSearchFilterError(_)
        ));
    }

    #[tokio::test]
    async fn should_include_filter_id_and_name_when_creating_command() {
        let product: Product = Faker.fake();
        let event = mk_event(&product);
        let product_clone = product.clone();
        let filter_id = UserSearchFilterId::new();
        let filter_name = UserSearchFilterName::from("My Antiques Filter");
        let summary = UserSearchFilterSummary {
            user_id: UserId::new(),
            user_search_filter_id: filter_id,
            name: filter_name.clone(),
            notifications: true,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        };

        let mut get_service = MockGetProductService::default();
        get_service
            .expect_find_product()
            .return_once(move |_, _| Box::pin(async move { Ok(product_clone) }));

        let mut filter_service = MockUserSearchFilterService::default();
        filter_service
            .expect_match_user_search_filters()
            .return_once(move |_| Box::pin(async move { Ok(vec![summary]) }));
        filter_service
            .expect_find_search_filter_product_match()
            .return_once(|_, _, _, _| Box::pin(async { Ok(None) }));

        let service =
            ProductEventSearchFilterNotificationsServiceImpl::new(&filter_service, &get_service);

        let result = service.determine_notification_commands(event).await;

        let cmds = result.unwrap();
        assert_eq!(cmds.len(), 1);
        match &cmds[0].notification_payload {
            NotificationPayload::SearchFilter {
                search_filter_payload,
                ..
            } => {
                assert_eq!(search_filter_payload.user_search_filter_id, filter_id);
                assert_eq!(search_filter_payload.user_search_filter_name, filter_name);
            }
            _ => panic!("Expected SearchFilter payload"),
        }
    }

    #[tokio::test]
    async fn should_include_product_fields_when_creating_command() {
        let product: Product = Faker.fake();
        let event = mk_event(&product);
        let product_clone = product.clone();
        let expected_product_id = product.product_id;
        let expected_shop_id = product.shop_id;
        let summary = mk_filter_summary(UserId::new());

        let mut get_service = MockGetProductService::default();
        get_service
            .expect_find_product()
            .return_once(move |_, _| Box::pin(async move { Ok(product_clone) }));

        let mut filter_service = MockUserSearchFilterService::default();
        filter_service
            .expect_match_user_search_filters()
            .return_once(move |_| Box::pin(async move { Ok(vec![summary]) }));
        filter_service
            .expect_find_search_filter_product_match()
            .return_once(|_, _, _, _| Box::pin(async { Ok(None) }));

        let service =
            ProductEventSearchFilterNotificationsServiceImpl::new(&filter_service, &get_service);

        let result = service.determine_notification_commands(event).await;

        let cmds = result.unwrap();
        match &cmds[0].notification_payload {
            NotificationPayload::SearchFilter {
                product_id,
                shop_id,
                ..
            } => {
                assert_eq!(*product_id, expected_product_id);
                assert_eq!(*shop_id, expected_shop_id);
            }
            _ => panic!("Expected SearchFilter payload"),
        }
    }

    #[tokio::test]
    async fn should_filter_out_already_matched_filters_for_determine_commands() {
        let product: Product = Faker.fake();
        let event = mk_event(&product);
        let product_clone = product.clone();
        let product_clone2 = product.clone();
        let user_id = UserId::new();
        let summary1 = mk_filter_summary(user_id);
        let summary1_filter_id = summary1.user_search_filter_id;
        let summary2 = mk_filter_summary(user_id);

        let mut get_service = MockGetProductService::default();
        get_service
            .expect_find_product()
            .return_once(move |_, _| Box::pin(async move { Ok(product_clone) }));

        let mut filter_service = MockUserSearchFilterService::default();
        filter_service
            .expect_match_user_search_filters()
            .return_once(move |_| Box::pin(async move { Ok(vec![summary1, summary2]) }));

        // First filter already matched (return Some), second not matched (return None)
        let shop_id = product_clone2.shop_id;
        let shops_product_id = product_clone2.shops_product_id.clone();
        filter_service
            .expect_find_search_filter_product_match()
            .withf(move |_, filter_id, sid, spid| {
                *filter_id == summary1_filter_id && *sid == shop_id && *spid == shops_product_id
            })
            .return_once(|_, _, _, _| Box::pin(async { Ok(Some(Faker.fake())) }));
        filter_service
            .expect_find_search_filter_product_match()
            .return_once(|_, _, _, _| Box::pin(async { Ok(None) }));

        let service =
            ProductEventSearchFilterNotificationsServiceImpl::new(&filter_service, &get_service);

        let result = service.determine_notification_commands(event).await;

        assert!(result.is_ok());
        let cmds = result.unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].user_id, user_id);
    }

    #[tokio::test]
    async fn should_return_empty_when_all_filters_already_matched() {
        let product: Product = Faker.fake();
        let event = mk_event(&product);
        let product_clone = product.clone();
        let user_id = UserId::new();
        let summary = mk_filter_summary(user_id);

        let mut get_service = MockGetProductService::default();
        get_service
            .expect_find_product()
            .return_once(move |_, _| Box::pin(async move { Ok(product_clone) }));

        let mut filter_service = MockUserSearchFilterService::default();
        filter_service
            .expect_match_user_search_filters()
            .return_once(move |_| Box::pin(async move { Ok(vec![summary]) }));
        filter_service
            .expect_find_search_filter_product_match()
            .return_once(|_, _, _, _| Box::pin(async { Ok(Some(Faker.fake())) }));

        let service =
            ProductEventSearchFilterNotificationsServiceImpl::new(&filter_service, &get_service);

        let result = service.determine_notification_commands(event).await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn should_return_commands_when_created_event_for_determine_commands() {
        let product: Product = Faker.fake();
        let event = mk_created_event(&product);
        let user_id = UserId::new();
        let summary = mk_filter_summary(user_id);

        let get_service = MockGetProductService::default();

        let mut filter_service = MockUserSearchFilterService::default();
        filter_service
            .expect_match_user_search_filters()
            .return_once(move |_| Box::pin(async move { Ok(vec![summary]) }));
        filter_service
            .expect_find_search_filter_product_match()
            .return_once(|_, _, _, _| Box::pin(async { Ok(None) }));

        let service =
            ProductEventSearchFilterNotificationsServiceImpl::new(&filter_service, &get_service);

        let result = service.determine_notification_commands(event).await;

        assert!(result.is_ok());
        let cmds = result.unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].user_id, user_id);
        assert!(cmds[0].external);
    }

    #[tokio::test]
    async fn should_not_call_get_product_service_when_event_is_created() {
        let product: Product = Faker.fake();
        let event = mk_created_event(&product);
        let user_id = UserId::new();
        let summary = mk_filter_summary(user_id);

        let get_service = MockGetProductService::default();
        let mut filter_service = MockUserSearchFilterService::default();
        filter_service
            .expect_match_user_search_filters()
            .return_once(move |_| Box::pin(async move { Ok(vec![summary]) }));
        filter_service
            .expect_find_search_filter_product_match()
            .return_once(|_, _, _, _| Box::pin(async { Ok(None) }));

        let service =
            ProductEventSearchFilterNotificationsServiceImpl::new(&filter_service, &get_service);

        let result = service.determine_notification_commands(event).await;

        assert!(result.is_ok());
        let cmds = result.unwrap();
        assert_eq!(cmds.len(), 1);
    }

    #[tokio::test]
    async fn should_include_created_product_fields_when_creating_command() {
        let product: Product = Faker.fake();
        let event = mk_created_event(&product);
        let expected_product_id = product.product_id;
        let expected_shop_id = product.shop_id;
        let expected_shop_name = product.shop_name.clone();
        let summary = mk_filter_summary(UserId::new());

        let get_service = MockGetProductService::default();

        let mut filter_service = MockUserSearchFilterService::default();
        filter_service
            .expect_match_user_search_filters()
            .return_once(move |_| Box::pin(async move { Ok(vec![summary]) }));
        filter_service
            .expect_find_search_filter_product_match()
            .return_once(|_, _, _, _| Box::pin(async { Ok(None) }));

        let service =
            ProductEventSearchFilterNotificationsServiceImpl::new(&filter_service, &get_service);

        let result = service.determine_notification_commands(event).await;

        let cmds = result.unwrap();
        assert_eq!(cmds.len(), 1);
        match &cmds[0].notification_payload {
            NotificationPayload::SearchFilter {
                product_id,
                shop_id,
                shop_name,
                ..
            } => {
                assert_eq!(*product_id, expected_product_id);
                assert_eq!(*shop_id, expected_shop_id);
                assert_eq!(*shop_name, expected_shop_name);
            }
            _ => panic!("Expected SearchFilter payload"),
        }
    }

    #[tokio::test]
    async fn should_return_empty_when_no_filters_match_created_event() {
        let product: Product = Faker.fake();
        let event = mk_created_event(&product);

        let get_service = MockGetProductService::default();

        let mut filter_service = MockUserSearchFilterService::default();
        filter_service
            .expect_match_user_search_filters()
            .return_once(|_| Box::pin(async { Ok(vec![]) }));

        let service =
            ProductEventSearchFilterNotificationsServiceImpl::new(&filter_service, &get_service);

        let result = service.determine_notification_commands(event).await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn should_filter_out_already_matched_filters_for_created_event() {
        let product: Product = Faker.fake();
        let event = mk_created_event(&product);
        let user_id = UserId::new();
        let summary1 = mk_filter_summary(user_id);
        let summary1_filter_id = summary1.user_search_filter_id;
        let summary2 = mk_filter_summary(user_id);
        let shop_id = product.shop_id;
        let shops_product_id = product.shops_product_id.clone();

        let get_service = MockGetProductService::default();

        let mut filter_service = MockUserSearchFilterService::default();
        filter_service
            .expect_match_user_search_filters()
            .return_once(move |_| Box::pin(async move { Ok(vec![summary1, summary2]) }));

        // First filter already matched (return Some), second not matched (return None)
        filter_service
            .expect_find_search_filter_product_match()
            .withf(move |_, filter_id, sid, spid| {
                *filter_id == summary1_filter_id && *sid == shop_id && *spid == shops_product_id
            })
            .return_once(|_, _, _, _| Box::pin(async { Ok(Some(Faker.fake())) }));
        filter_service
            .expect_find_search_filter_product_match()
            .return_once(|_, _, _, _| Box::pin(async { Ok(None) }));

        let service =
            ProductEventSearchFilterNotificationsServiceImpl::new(&filter_service, &get_service);

        let result = service.determine_notification_commands(event).await;

        assert!(result.is_ok());
        let cmds = result.unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].user_id, user_id);
    }
}
