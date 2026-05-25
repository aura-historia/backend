use common::enhanced_match_reason::EnhancedMatchReason;
use common::language::domain::Language;
use common::pagination::cursor::Cursor;
use common::resource_state::domain::ResourceState;
use notification::core::notification::{NotificationPayload, NotificationSearchFilterPayload};
use notification::service::command::CreateNotificationCommand;
use notification::service::notification_service::NotificationService;
use product::core::description::Description;
use product::core::product::LocalizedProductView;
use product::core::product_search::ProductSearch;
use product::core::sort_product_field::SortProductField;
use product::service::query_service::{QueryProductService, SearchProductsError};
use product_pipeline_embed_text::service::{MultimodalEmbeddingError, MultimodalEmbeddingService};
use search_filter::core::quota::SearchFilterQuota;
use search_filter::core::search_filter_product_match::SearchFilterProductMatch;
use search_filter::core::user_search_filter::UserSearchFilter;
use search_filter::core::user_search_filter_search::UserSearchFilterSearch;
use search_filter::service::enhanced_search_match_service::{
    EnhancedSearchMatchError, EnhancedSearchMatchService,
};
use search_filter::service::user_search_filter_service::{
    UserSearchFilterError, UserSearchFilterService,
};
use time::{Duration, OffsetDateTime};
use tracing::{debug, warn};
use user::service::user_service::{UserService, UserServiceError};

const FILTER_PAGE_SIZE: u64 = 100;
const PRODUCT_PAGE_SIZE: u64 = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeriodicMatcherResult {
    pub filters_processed: usize,
    pub matches_created: usize,
    pub notifications_created: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum PeriodicMatcherError {
    #[error("UserSearchFilterError: {0}")]
    UserSearchFilterError(#[from] UserSearchFilterError),
    #[error("SearchProductsError: {0}")]
    SearchProductsError(#[from] SearchProductsError),
    #[error("MultimodalEmbeddingError: {0}")]
    MultimodalEmbeddingError(#[from] MultimodalEmbeddingError),
    #[error("EnhancedSearchMatchError: {0}")]
    EnhancedSearchMatchError(#[from] EnhancedSearchMatchError),
    #[error("UserServiceError: {0}")]
    UserServiceError(#[from] UserServiceError),
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait PeriodicMatcherService {
    async fn match_active_filters(&self) -> Result<PeriodicMatcherResult, PeriodicMatcherError>;
}

pub struct PeriodicMatcherServiceImpl<'a> {
    user_search_filter_service: &'a (dyn UserSearchFilterService + Sync),
    query_product_service: &'a (dyn QueryProductService + Sync),
    embedding_service: &'a (dyn MultimodalEmbeddingService + Sync + Send),
    enhanced_search_match_service: &'a (dyn EnhancedSearchMatchService + Sync + Send),
    notification_service: &'a (dyn NotificationService + Sync),
    user_service: &'a (dyn UserService + Sync),
}

impl<'a> PeriodicMatcherServiceImpl<'a> {
    pub fn new(
        user_search_filter_service: &'a (dyn UserSearchFilterService + Sync),
        query_product_service: &'a (dyn QueryProductService + Sync),
        embedding_service: &'a (dyn MultimodalEmbeddingService + Sync + Send),
        enhanced_search_match_service: &'a (dyn EnhancedSearchMatchService + Sync + Send),
        notification_service: &'a (dyn NotificationService + Sync),
        user_service: &'a (dyn UserService + Sync),
    ) -> Self {
        Self {
            user_search_filter_service,
            query_product_service,
            embedding_service,
            enhanced_search_match_service,
            notification_service,
            user_service,
        }
    }

    async fn process_filter(
        &self,
        filter: UserSearchFilter,
    ) -> Result<(usize, usize), PeriodicMatcherError> {
        let matched_at = OffsetDateTime::now_utc();
        let product_search =
            search_since_last_match(&filter.search, filter.last_hybrid_search_matched);
        let embedding = match product_search.product_query.as_ref() {
            Some(query) if !query.as_ref().trim().is_empty() => {
                Some(self.embedding_service.embed_query(query.as_ref()).await?)
            }
            _ => None,
        };

        let mut cursor = Some(Cursor {
            size: PRODUCT_PAGE_SIZE,
            search_after: None,
        });
        let mut matches_created = 0;
        let mut notifications_created = 0;

        loop {
            let products = match embedding.as_ref() {
                Some(embedding) => {
                    self.query_product_service
                        .search_products_with_dynamic_semantics(&product_search, embedding, &cursor)
                        .await?
                }
                None => {
                    self.query_product_service
                        .search_products(
                            &product_search,
                            &None::<common::sort::Sort<SortProductField>>,
                            &cursor,
                        )
                        .await?
                }
            };

            if products.items.is_empty() {
                break;
            }

            let (matches, notifications) = self
                .match_products_for_filter(&filter, products.items)
                .await?;

            if !matches.is_empty() {
                let result = self
                    .user_search_filter_service
                    .create_search_filter_product_matches(matches)
                    .await?;
                if !result.unprocessed.is_empty() {
                    return Err(UserSearchFilterError::SdkBatchWriteItemError(
                        aws_sdk_dynamodb::error::SdkError::construction_failure(
                            "Failed writing all periodic hybrid search-filter matches.",
                        ),
                    )
                    .into());
                }
                matches_created += result.processed.len();
            }

            if !notifications.is_empty() {
                let notification_result = self
                    .notification_service
                    .create_notifications(&common::event_id::EventId::new(), notifications)
                    .await;
                if !notification_result.unprocessed.is_empty() {
                    return Err(UserSearchFilterError::SdkBatchWriteItemError(
                        aws_sdk_dynamodb::error::SdkError::construction_failure(
                            "Failed writing all periodic hybrid search-filter notifications.",
                        ),
                    )
                    .into());
                }
                notifications_created += notification_result.processed.len();
            }

            let next_cursor = products.cursor;
            if next_cursor.search_after.is_none() || next_cursor.size == 0 {
                break;
            }
            cursor = Some(next_cursor);
        }

        self.user_search_filter_service
            .update_user_search_filter_last_hybrid_search_matched(
                &filter.user_id,
                &filter.user_search_filter_id,
                matched_at,
            )
            .await?;

        Ok((matches_created, notifications_created))
    }

    async fn match_products_for_filter(
        &self,
        filter: &UserSearchFilter,
        products: Vec<LocalizedProductView>,
    ) -> Result<
        (
            Vec<SearchFilterProductMatch>,
            Vec<CreateNotificationCommand>,
        ),
        PeriodicMatcherError,
    > {
        let mut matches = Vec::new();
        let mut notifications = Vec::new();
        let now = OffsetDateTime::now_utc();
        let quota_allows_notification = self.user_allows_notification(filter).await?;

        for product in products {
            let existing = self
                .user_search_filter_service
                .find_search_filter_product_match(
                    &filter.user_id,
                    &filter.user_search_filter_id,
                    &product.shop_id,
                    &product.shops_product_id,
                )
                .await?;
            if existing.is_some() {
                debug!(
                    userId = %filter.user_id,
                    searchFilterId = %filter.user_search_filter_id,
                    shopId = %product.shop_id,
                    shopsProductId = %product.shops_product_id,
                    "Skipping already-matched search filter for product."
                );
                continue;
            }

            let enhanced_match_reason = self.enhanced_match_reason(filter, &product).await?;
            matches.push(SearchFilterProductMatch {
                user_id: filter.user_id,
                user_search_filter_id: filter.user_search_filter_id,
                user_search_filter_name: Some(filter.name.clone()),
                shop_id: product.shop_id,
                shops_product_id: product.shops_product_id.clone(),
                product_id: product.product_id,
                origin_event_id: product.event_id,
                enhanced_match_reason: enhanced_match_reason.clone(),
                feedback: None,
                created: now,
                updated: now,
            });

            if quota_allows_notification {
                notifications.push(mk_notification_command(&product, filter));
            }
        }

        Ok((matches, notifications))
    }

    async fn enhanced_match_reason(
        &self,
        filter: &UserSearchFilter,
        product: &LocalizedProductView,
    ) -> Result<Option<EnhancedMatchReason>, PeriodicMatcherError> {
        let Some(enhanced_description) = filter.enhanced_search_description.as_ref() else {
            return Ok(None);
        };

        let language = self
            .user_service
            .find_user(&filter.user_id)
            .await
            .map(|user| user.language.unwrap_or(Language::En))
            .unwrap_or(Language::En);
        let description = product
            .description
            .as_ref()
            .map(|description| description.payload.clone())
            .unwrap_or_else(|| Description::from(""));
        let images: Vec<_> = product.images.iter().take(5).cloned().collect();

        match self
            .enhanced_search_match_service
            .evaluate(
                enhanced_description,
                &product.title.payload,
                &description,
                language,
                &images,
            )
            .await
        {
            Ok(result) if result.matches => Ok(result.reason),
            Ok(_) => Ok(None),
            Err(err) => {
                warn!(
                    userId = %filter.user_id,
                    searchFilterId = %filter.user_search_filter_id,
                    error = %err,
                    "Enhanced search match evaluation failed. Including filter without reason."
                );
                Ok(None)
            }
        }
    }

    async fn user_allows_notification(
        &self,
        filter: &UserSearchFilter,
    ) -> Result<bool, PeriodicMatcherError> {
        let user = self.user_service.find_user(&filter.user_id).await?;
        let match_count = self
            .user_search_filter_service
            .count_user_search_filter_matches_for_this_month(&filter.user_id)
            .await?;
        Ok((match_count as u32) < user.tier.search_filter_match_quota())
    }
}

#[async_trait::async_trait]
impl<'a> PeriodicMatcherService for PeriodicMatcherServiceImpl<'a> {
    async fn match_active_filters(&self) -> Result<PeriodicMatcherResult, PeriodicMatcherError> {
        let search = UserSearchFilterSearch {
            state: Some(ResourceState::Active),
        };
        let mut cursor = Some(Cursor {
            size: FILTER_PAGE_SIZE,
            search_after: None,
        });
        let mut result = PeriodicMatcherResult {
            filters_processed: 0,
            matches_created: 0,
            notifications_created: 0,
        };

        loop {
            let page = self
                .user_search_filter_service
                .query_user_search_filters(&search, &cursor)
                .await?;
            if page.items.is_empty() {
                break;
            }

            for filter in page.items {
                let (matches_created, notifications_created) = self.process_filter(filter).await?;
                result.filters_processed += 1;
                result.matches_created += matches_created;
                result.notifications_created += notifications_created;
            }

            if page.cursor.search_after.is_none() || page.cursor.size == 0 {
                break;
            }
            cursor = Some(page.cursor);
        }

        Ok(result)
    }
}

fn search_since_last_match(search: &ProductSearch, last_matched: OffsetDateTime) -> ProductSearch {
    let mut search = search.clone();
    let min = last_matched + Duration::NANOSECOND;
    search.updated_query = Some(match search.updated_query {
        Some(mut query) => {
            query.min = Some(query.min.map_or(min, |existing| existing.max(min)));
            query
        }
        None => common::query::range_query::RangeQuery {
            min: Some(min),
            max: None,
        },
    });
    search
}

fn mk_notification_command(
    product: &LocalizedProductView,
    filter: &UserSearchFilter,
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
            title: [(product.title.localization, product.title.payload.clone())].into(),
            image: product.images.first().cloned(),
            url: product.url.clone(),
            view_url: product.view_url.clone(),
            search_filter_payload: NotificationSearchFilterPayload {
                user_search_filter_id: filter.user_search_filter_id,
                user_search_filter_name: filter.name.clone(),
            },
        },
        external: filter.notifications,
    }
}
