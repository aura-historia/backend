use crate::ports::SearchFilterMatchNotificationSource;
use common::error::boxed::{BoxError, box_error};
use notification_core::notification::{NotificationPayload, NotificationSearchFilterPayload};
use notification_service::use_cases::commands::create_notification::{
    CreateNotificationCommand, CreateNotificationUseCase,
};
use product_service::ports::ProductSearchFilterMatchSource;

#[derive(Debug, Clone, PartialEq)]
pub struct GenerateSearchFilterMatchNotificationCommand {
    pub match_source: SearchFilterMatchNotificationSource,
    pub product: ProductSearchFilterMatchSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerateSearchFilterMatchNotificationResult;

#[derive(Debug, thiserror::Error)]
pub enum GenerateSearchFilterMatchNotificationError {
    #[error("search filter match notification creation failed")]
    NotificationCreateFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait GenerateSearchFilterMatchNotificationUseCase: Send + Sync {
    async fn execute(
        &self,
        command: GenerateSearchFilterMatchNotificationCommand,
    ) -> Result<
        GenerateSearchFilterMatchNotificationResult,
        GenerateSearchFilterMatchNotificationError,
    >;
}

pub struct GenerateSearchFilterMatchNotificationHandler<N> {
    notifications: N,
}

impl<N> GenerateSearchFilterMatchNotificationHandler<N> {
    pub fn new(notifications: N) -> Self {
        Self { notifications }
    }
}

#[async_trait::async_trait]
impl<N> GenerateSearchFilterMatchNotificationUseCase
    for GenerateSearchFilterMatchNotificationHandler<N>
where
    N: CreateNotificationUseCase,
{
    #[tracing::instrument(
        name = "generate_search_filter_match_notification",
        skip_all,
        fields(
            origin_event_id = %command.match_source.origin_event_id,
            product_id = %command.product.product_id,
            user_id = %command.match_source.user_id,
            search_filter_id = %command.match_source.search_filter_id,
        )
    )]
    async fn execute(
        &self,
        command: GenerateSearchFilterMatchNotificationCommand,
    ) -> Result<
        GenerateSearchFilterMatchNotificationResult,
        GenerateSearchFilterMatchNotificationError,
    > {
        let match_source = command.match_source;
        let product = command.product;
        self.notifications
            .execute(CreateNotificationCommand {
                user_id: match_source.user_id,
                origin_event_id: match_source.origin_event_id,
                notification_payload: NotificationPayload::SearchFilter {
                    product_id: product.product_id,
                    shop_id: product.shop_id,
                    shops_product_id: product.shops_product_id,
                    shop_slug_id: product.shop_slug_id,
                    product_slug_id: product.product_slug_id,
                    shop_name: product.shop_name,
                    title: (!product.titles.is_empty()).then_some(product.titles),
                    image: product.image,
                    url: product.url,
                    view_url: product.view_url,
                    search_filter_payload: NotificationSearchFilterPayload {
                        user_search_filter_id: match_source.search_filter_id,
                        user_search_filter_name: match_source.search_filter_name,
                    },
                },
                external: match_source.external,
            })
            .await
            .map_err(|source| {
                GenerateSearchFilterMatchNotificationError::NotificationCreateFailed {
                    source: box_error(source),
                }
            })?;

        Ok(GenerateSearchFilterMatchNotificationResult)
    }
}
