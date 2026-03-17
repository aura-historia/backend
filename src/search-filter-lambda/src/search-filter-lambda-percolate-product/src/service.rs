use common::has_key::HasKey;
use notification::core::notification::{NotificationPayload, NotificationSearchFilterPayload};
use notification::service::command::CreateNotificationCommand;
use product::core::product::Product;
use product::core::product_event::ProductEvent;
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

        let mut product = self
            .get_product_service
            .find_product(&product_key.shop_id, &product_key.shops_product_id)
            .await?;

        product.apply(event);

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

        let commands = matched_filters
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
    use common::product_id::ProductId;
    use common::product_state::domain::ProductState;
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use product::core::product_event::ProductEventPayload;
    use product::core::product_event::domain::{
        ProductDomainEventPayload, ProductStateChangeDomainEventPayload,
    };
    use product::service::get_service::MockGetProductService;
    use search_filter::core::user_search_filter::UserSearchFilterSummary;
    use search_filter::core::user_search_filter_id::UserSearchFilterId;
    use search_filter::core::user_search_filter_name::UserSearchFilterName;
    use search_filter::service::user_search_filter_service::MockUserSearchFilterService;
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
}
