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
use search_filter::core::user_search_filter_update::UserSearchFilterUpdate;
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
            .update_user_search_filter(
                &filter.user_id,
                &filter.user_search_filter_id,
                UserSearchFilterUpdate {
                    updated: matched_at,
                    last_hybrid_search_matched: Some(matched_at),
                    ..Default::default()
                },
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
                .search_user_search_filters(&search, &cursor)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::PeriodicMatcherServiceImpl;
    use common::event_id::EventId;
    use common::pagination::cursor::CursoredResult;
    use common::query::range_query::RangeQuery;
    use fake::{Fake, Faker};
    use notification::core::{
        notification::Notification,
        notification_id::NotificationId,
        notification::NotificationPayload,
        notification_type::NotificationType,
    };
    use notification::service::notification_service::{
        CreateNotificationsResult, MockNotificationService,
    };
    use product::service::query_service::MockQueryProductService;
    use product_pipeline_embed_text::service::MockMultimodalEmbeddingService;
    use search_filter::core::quota::SearchFilterQuota;
    use search_filter::core::user_search_filter::EnhancedSearchDescription;
    use search_filter::service::enhanced_search_match_service::{
        EnhancedSearchMatchResult, MockEnhancedSearchMatchService,
    };
    use search_filter::service::user_search_filter_service::{
        CreateSearchFilterProductMatchesResult, MockUserSearchFilterService,
    };
    use serde_json::json;
    use time::macros::datetime;
    use user::core::{tier::UserTier, user::User};
    use user::service::user_service::MockUserService;

    fn mk_filter() -> UserSearchFilter {
        let mut filter: UserSearchFilter = Faker.fake();
        filter.notifications = true;
        filter.search.language = Language::En;
        filter.search.currency = common::currency::domain::Currency::Eur;
        filter.search.product_query = None;
        filter.enhanced_search_description = None;
        filter.last_hybrid_search_matched = datetime!(2026-05-10 12:00 UTC);
        filter
    }

    fn mk_product() -> LocalizedProductView {
        let mut product: LocalizedProductView = Faker.fake();
        product.images.truncate(2);
        product
    }

    fn mk_user(tier: UserTier) -> User {
        let mut user: User = Faker.fake();
        user.tier = tier;
        user.language = Some(Language::En);
        user
    }

    fn mk_notification(origin_event_id: EventId) -> Notification {
        Notification {
            user_id: Faker.fake(),
            origin_event_id,
            notification_id: NotificationId::new(),
            notification_type: Some(NotificationType::Email),
            notification_payload: Faker.fake::<NotificationPayload>(),
            seen: false,
            external: true,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        }
    }

    fn mk_search_result(
        items: Vec<LocalizedProductView>,
        search_after: Option<serde_json::Value>,
    ) -> CursoredResult<LocalizedProductView, serde_json::Value> {
        CursoredResult {
            total: Some(items.len() as u64),
            cursor: Cursor {
                size: items.len() as u64,
                search_after,
            },
            items,
        }
    }

    #[test]
    fn should_add_updated_min_after_last_match_when_none_exists() {
        let search = product::core::product_search::ProductSearch::default();
        let last_matched = datetime!(2026-05-10 12:00 UTC);

        let updated = search_since_last_match(&search, last_matched);

        assert_eq!(
            updated.updated_query,
            Some(RangeQuery {
                min: Some(last_matched + Duration::NANOSECOND),
                max: None,
            })
        );
        assert_eq!(search.updated_query, None);
    }

    #[test]
    fn should_keep_later_updated_min_when_search_already_has_one() {
        let min = datetime!(2026-05-01 00:00 UTC);
        let last_matched = datetime!(2026-05-10 12:00 UTC);
        let mut search = product::core::product_search::ProductSearch::default();
        search.updated_query = Some(RangeQuery {
            min: Some(min),
            max: Some(datetime!(2026-06-01 00:00 UTC)),
        });

        let updated = search_since_last_match(&search, last_matched);

        assert_eq!(
            updated.updated_query.unwrap().min,
            Some(last_matched + Duration::NANOSECOND)
        );
    }

    #[test]
    fn should_build_notification_command_from_filter_and_product() {
        let filter = mk_filter();
        let product = mk_product();

        let cmd = mk_notification_command(&product, &filter);

        assert_eq!(cmd.user_id, filter.user_id);
        assert!(cmd.external);
        match cmd.notification_payload {
            NotificationPayload::SearchFilter {
                product_id,
                shop_id,
                shops_product_id,
                search_filter_payload,
                ..
            } => {
                assert_eq!(product_id, product.product_id);
                assert_eq!(shop_id, product.shop_id);
                assert_eq!(shops_product_id, product.shops_product_id);
                assert_eq!(search_filter_payload.user_search_filter_id, filter.user_search_filter_id);
                assert_eq!(search_filter_payload.user_search_filter_name, filter.name);
            }
            payload => panic!("expected search-filter payload, got {payload:?}"),
        }
    }

    #[tokio::test]
    async fn should_update_watermark_when_no_products_match_for_filter() {
        let filter = mk_filter();
        let expected_user_id = filter.user_id;
        let expected_filter_id = filter.user_search_filter_id;
        let expected_min = filter.last_hybrid_search_matched + Duration::NANOSECOND;

        let mut filter_service = MockUserSearchFilterService::default();
        filter_service.expect_update_user_search_filter().return_once(
            move |user_id, filter_id, update| {
                let actual_user_id = *user_id;
                let actual_filter_id = *filter_id;
                Box::pin(async move {
                    assert_eq!(actual_user_id, expected_user_id);
                    assert_eq!(actual_filter_id, expected_filter_id);
                    assert_eq!(update.last_hybrid_search_matched, Some(update.updated));
                    Ok(Faker.fake())
                })
            },
        );

        let mut query_service = MockQueryProductService::default();
        query_service
            .expect_search_products()
            .return_once(move |search, _, cursor| {
                assert_eq!(search.updated_query.as_ref().and_then(|range| range.min), Some(expected_min));
                assert_eq!(
                    *cursor,
                    Some(Cursor {
                        size: PRODUCT_PAGE_SIZE,
                        search_after: None,
                    })
                );
                Box::pin(async { Ok(CursoredResult::default()) })
            });
        query_service
            .expect_search_products_with_dynamic_semantics()
            .times(0);

        let mut embedding_service = MockMultimodalEmbeddingService::default();
        embedding_service.expect_embed_query().times(0);

        let enhanced_service = MockEnhancedSearchMatchService::default();
        let notification_service = MockNotificationService::default();
        let user_service = MockUserService::default();

        let service = PeriodicMatcherServiceImpl::new(
            &filter_service,
            &query_service,
            &embedding_service,
            &enhanced_service,
            &notification_service,
            &user_service,
        );

        let result = service.process_filter(filter).await.unwrap();

        assert_eq!(result, (0, 0));
    }

    #[tokio::test]
    async fn should_use_semantic_search_and_create_match_and_notification() {
        let mut filter = mk_filter();
        filter.search.product_query = Some("gold ring".try_into().unwrap());
        let product = mk_product();
        let product_for_match = product.clone();
        let origin_event_id = product.event_id;
        let user = mk_user(UserTier::Free);

        let mut filter_service = MockUserSearchFilterService::default();
        filter_service
            .expect_find_search_filter_product_match()
            .return_once(|_, _, _, _| Box::pin(async { Ok(None) }));
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .return_once(|_| Box::pin(async { Ok(0) }));
        filter_service
            .expect_create_search_filter_product_matches()
            .return_once(move |matches| {
                Box::pin(async move {
                    assert_eq!(matches.len(), 1);
                    assert_eq!(matches[0].origin_event_id, product_for_match.event_id);
                    Ok(CreateSearchFilterProductMatchesResult {
                        processed: matches,
                        unprocessed: vec![],
                    })
                })
            });
        filter_service.expect_update_user_search_filter().return_once(
            |_, _, update| Box::pin(async move {
                assert_eq!(update.last_hybrid_search_matched, Some(update.updated));
                Ok(Faker.fake())
            }),
        );

        let mut query_service = MockQueryProductService::default();
        query_service.expect_search_products().times(0);
        query_service
            .expect_search_products_with_dynamic_semantics()
            .return_once(move |search, embedding, cursor| {
                assert_eq!(
                    search.product_query.as_ref().map(|query| query.as_ref()),
                    Some("gold ring")
                );
                assert_eq!(embedding, &[0.1_f32, 0.2_f32, 0.3_f32]);
                assert_eq!(
                    *cursor,
                    Some(Cursor {
                        size: PRODUCT_PAGE_SIZE,
                        search_after: None,
                    })
                );
                let product = product.clone();
                Box::pin(async move { Ok(mk_search_result(vec![product], None)) })
            });

        let mut embedding_service = MockMultimodalEmbeddingService::default();
        embedding_service
            .expect_embed_query()
            .return_once(|query| {
                assert_eq!(query, "gold ring");
                Box::pin(async { Ok(vec![0.1, 0.2, 0.3]) })
            });

        let enhanced_service = MockEnhancedSearchMatchService::default();

        let mut notification_service = MockNotificationService::default();
        notification_service
            .expect_create_notifications()
            .return_once(move |_event_id, cmds| {
                Box::pin(async move {
                    assert_eq!(cmds.len(), 1);
                    assert_eq!(cmds[0].user_id, filter.user_id);
                    CreateNotificationsResult {
                        unprocessed: vec![],
                        processed: vec![mk_notification(origin_event_id)],
                    }
                })
            });

        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .return_once(move |_| Box::pin(async move { Ok(user) }));

        let service = PeriodicMatcherServiceImpl::new(
            &filter_service,
            &query_service,
            &embedding_service,
            &enhanced_service,
            &notification_service,
            &user_service,
        );

        let result = service.process_filter(filter).await.unwrap();

        assert_eq!(result, (1, 1));
    }

    #[tokio::test]
    async fn should_skip_existing_product_matches() {
        let filter = mk_filter();
        let product = mk_product();

        let mut filter_service = MockUserSearchFilterService::default();
        filter_service
            .expect_find_search_filter_product_match()
            .return_once(|_, _, _, _| Box::pin(async { Ok(Some(Faker.fake())) }));
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .return_once(|_| Box::pin(async { Ok(0) }));

        let query_service = MockQueryProductService::default();
        let embedding_service = MockMultimodalEmbeddingService::default();
        let enhanced_service = MockEnhancedSearchMatchService::default();
        let notification_service = MockNotificationService::default();
        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .return_once(|_| Box::pin(async { Ok(mk_user(UserTier::Free)) }));

        let service = PeriodicMatcherServiceImpl::new(
            &filter_service,
            &query_service,
            &embedding_service,
            &enhanced_service,
            &notification_service,
            &user_service,
        );

        let (matches, notifications) = service
            .match_products_for_filter(&filter, vec![product])
            .await
            .unwrap();

        assert!(matches.is_empty());
        assert!(notifications.is_empty());
    }

    #[tokio::test]
    async fn should_return_none_for_enhanced_reason_when_filter_has_no_enhanced_description() {
        let filter = mk_filter();
        let product = mk_product();

        let filter_service = MockUserSearchFilterService::default();
        let query_service = MockQueryProductService::default();
        let embedding_service = MockMultimodalEmbeddingService::default();
        let enhanced_service = MockEnhancedSearchMatchService::default();
        let notification_service = MockNotificationService::default();
        let user_service = MockUserService::default();

        let service = PeriodicMatcherServiceImpl::new(
            &filter_service,
            &query_service,
            &embedding_service,
            &enhanced_service,
            &notification_service,
            &user_service,
        );

        let reason = service.enhanced_match_reason(&filter, &product).await.unwrap();

        assert_eq!(reason, None);
    }

    #[tokio::test]
    async fn should_return_reason_when_enhanced_match_succeeds() {
        let mut filter = mk_filter();
        filter.enhanced_search_description = Some(EnhancedSearchDescription::from("gold"));
        let product = mk_product();
        let expected_reason = Some(EnhancedMatchReason::from("close match"));

        let filter_service = MockUserSearchFilterService::default();
        let query_service = MockQueryProductService::default();
        let embedding_service = MockMultimodalEmbeddingService::default();
        let notification_service = MockNotificationService::default();

        let mut enhanced_service = MockEnhancedSearchMatchService::default();
        enhanced_service.expect_evaluate().return_once(move |_, _, _, _, _| {
            let reason = expected_reason.clone();
            Box::pin(async move {
                Ok(EnhancedSearchMatchResult {
                    matches: true,
                    reason,
                })
            })
        });

        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .return_once(|_| Box::pin(async { Ok(mk_user(UserTier::Free)) }));

        let service = PeriodicMatcherServiceImpl::new(
            &filter_service,
            &query_service,
            &embedding_service,
            &enhanced_service,
            &notification_service,
            &user_service,
        );

        let reason = service.enhanced_match_reason(&filter, &product).await.unwrap();

        assert_eq!(reason, Some(EnhancedMatchReason::from("close match")));
    }

    #[tokio::test]
    async fn should_return_none_when_enhanced_match_does_not_match() {
        let mut filter = mk_filter();
        filter.enhanced_search_description = Some(EnhancedSearchDescription::from("gold"));
        let product = mk_product();

        let filter_service = MockUserSearchFilterService::default();
        let query_service = MockQueryProductService::default();
        let embedding_service = MockMultimodalEmbeddingService::default();
        let notification_service = MockNotificationService::default();

        let mut enhanced_service = MockEnhancedSearchMatchService::default();
        enhanced_service.expect_evaluate().return_once(|_, _, _, _, _| {
            Box::pin(async {
                Ok(EnhancedSearchMatchResult {
                    matches: false,
                    reason: None,
                })
            })
        });

        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .return_once(|_| Box::pin(async { Ok(mk_user(UserTier::Free)) }));

        let service = PeriodicMatcherServiceImpl::new(
            &filter_service,
            &query_service,
            &embedding_service,
            &enhanced_service,
            &notification_service,
            &user_service,
        );

        let reason = service.enhanced_match_reason(&filter, &product).await.unwrap();

        assert_eq!(reason, None);
    }

    #[tokio::test]
    async fn should_ignore_enhanced_match_errors_and_continue_without_reason() {
        let mut filter = mk_filter();
        filter.enhanced_search_description = Some(EnhancedSearchDescription::from("gold"));
        let product = mk_product();

        let filter_service = MockUserSearchFilterService::default();
        let query_service = MockQueryProductService::default();
        let embedding_service = MockMultimodalEmbeddingService::default();
        let notification_service = MockNotificationService::default();

        let mut enhanced_service = MockEnhancedSearchMatchService::default();
        enhanced_service.expect_evaluate().return_once(|_, _, _, _, _| {
            Box::pin(async {
                Err(EnhancedSearchMatchError::InvalidResponse(
                    "bad llm response".to_string(),
                ))
            })
        });

        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .return_once(|_| Box::pin(async { Ok(mk_user(UserTier::Free)) }));

        let service = PeriodicMatcherServiceImpl::new(
            &filter_service,
            &query_service,
            &embedding_service,
            &enhanced_service,
            &notification_service,
            &user_service,
        );

        let reason = service.enhanced_match_reason(&filter, &product).await.unwrap();

        assert_eq!(reason, None);
    }

    #[tokio::test]
    async fn should_paginate_filters_and_accumulate_results() {
        let first_filter = mk_filter();
        let second_filter = mk_filter();
        let page_counter = std::sync::Arc::new(std::sync::Mutex::new(0usize));

        let mut filter_service = MockUserSearchFilterService::default();
        filter_service
            .expect_search_user_search_filters()
            .times(2)
            .returning(move |search, cursor| {
                let mut page = page_counter.lock().unwrap();
                let result = match *page {
                    0 => {
                        assert_eq!(search.state, Some(ResourceState::Active));
                        assert_eq!(
                            *cursor,
                            Some(Cursor {
                                size: FILTER_PAGE_SIZE,
                                search_after: None,
                            })
                        );
                        CursoredResult {
                            items: vec![first_filter.clone()],
                            cursor: Cursor {
                                size: 1,
                                search_after: Some(json!(["cursor-1"])),
                            },
                            total: Some(2),
                        }
                    }
                    1 => {
                        assert_eq!(search.state, Some(ResourceState::Active));
                        assert_eq!(
                            *cursor,
                            Some(Cursor {
                                size: 1,
                                search_after: Some(json!(["cursor-1"])),
                            })
                        );
                        CursoredResult {
                            items: vec![second_filter.clone()],
                            cursor: Cursor {
                                size: 1,
                                search_after: None,
                            },
                            total: Some(2),
                        }
                    }
                    _ => unreachable!("unexpected extra page"),
                };
                *page += 1;
                Box::pin(async move { Ok(result) })
            });
        filter_service
            .expect_update_user_search_filter()
            .times(2)
            .returning(|_, _, _| Box::pin(async { Ok(Faker.fake()) }));

        let mut query_service = MockQueryProductService::default();
        query_service
            .expect_search_products()
            .times(2)
            .returning(|_, _, _| Box::pin(async { Ok(CursoredResult::default()) }));
        query_service
            .expect_search_products_with_dynamic_semantics()
            .times(0);

        let mut embedding_service = MockMultimodalEmbeddingService::default();
        embedding_service.expect_embed_query().times(0);

        let enhanced_service = MockEnhancedSearchMatchService::default();
        let notification_service = MockNotificationService::default();
        let user_service = MockUserService::default();

        let service = PeriodicMatcherServiceImpl::new(
            &filter_service,
            &query_service,
            &embedding_service,
            &enhanced_service,
            &notification_service,
            &user_service,
        );

        let result = service.match_active_filters().await.unwrap();

        assert_eq!(
            result,
            PeriodicMatcherResult {
                filters_processed: 2,
                matches_created: 0,
                notifications_created: 0,
            }
        );
    }

    #[tokio::test]
    async fn should_fail_when_match_batch_write_leaves_unprocessed_items() {
        let filter = mk_filter();
        let product = mk_product();

        let mut filter_service = MockUserSearchFilterService::default();
        filter_service
            .expect_find_search_filter_product_match()
            .return_once(|_, _, _, _| Box::pin(async { Ok(None) }));
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .return_once(|_| Box::pin(async { Ok(UserTier::Free.search_filter_match_quota() as u64) }));
        filter_service
            .expect_create_search_filter_product_matches()
            .return_once(move |matches| {
                let unprocessed = matches.clone();
                Box::pin(async move {
                    Ok(CreateSearchFilterProductMatchesResult {
                        processed: vec![],
                        unprocessed,
                    })
                })
            });
        filter_service.expect_update_user_search_filter().times(0);

        let mut query_service = MockQueryProductService::default();
        query_service
            .expect_search_products()
            .return_once(move |_, _, _| {
                let product = product.clone();
                Box::pin(async move { Ok(mk_search_result(vec![product], None)) })
            });
        query_service
            .expect_search_products_with_dynamic_semantics()
            .times(0);

        let mut embedding_service = MockMultimodalEmbeddingService::default();
        embedding_service.expect_embed_query().times(0);

        let enhanced_service = MockEnhancedSearchMatchService::default();
        let notification_service = MockNotificationService::default();
        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .return_once(|_| Box::pin(async { Ok(mk_user(UserTier::Free)) }));

        let service = PeriodicMatcherServiceImpl::new(
            &filter_service,
            &query_service,
            &embedding_service,
            &enhanced_service,
            &notification_service,
            &user_service,
        );

        let result = service.process_filter(filter).await;

        assert!(matches!(
            result,
            Err(PeriodicMatcherError::UserSearchFilterError(
                UserSearchFilterError::SdkBatchWriteItemError(_)
            ))
        ));
    }

    #[tokio::test]
    async fn should_fail_when_notification_batch_write_leaves_unprocessed_items() {
        let filter = mk_filter();
        let product = mk_product();

        let mut filter_service = MockUserSearchFilterService::default();
        filter_service
            .expect_find_search_filter_product_match()
            .return_once(|_, _, _, _| Box::pin(async { Ok(None) }));
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .return_once(|_| Box::pin(async { Ok(0) }));
        filter_service
            .expect_create_search_filter_product_matches()
            .return_once(move |matches| {
                Box::pin(async move {
                    Ok(CreateSearchFilterProductMatchesResult {
                        processed: matches,
                        unprocessed: vec![],
                    })
                })
            });
        filter_service.expect_update_user_search_filter().times(0);

        let mut query_service = MockQueryProductService::default();
        query_service
            .expect_search_products()
            .return_once(move |_, _, _| {
                let product = product.clone();
                Box::pin(async move { Ok(mk_search_result(vec![product], None)) })
            });
        query_service
            .expect_search_products_with_dynamic_semantics()
            .times(0);

        let mut embedding_service = MockMultimodalEmbeddingService::default();
        embedding_service.expect_embed_query().times(0);

        let enhanced_service = MockEnhancedSearchMatchService::default();
        let mut notification_service = MockNotificationService::default();
        notification_service
            .expect_create_notifications()
            .return_once(|_, cmds| {
                Box::pin(async move {
                    CreateNotificationsResult {
                        unprocessed: cmds
                            .into_iter()
                            .map(|cmd| {
                                (
                                    cmd,
                                    notification::service::notification_service::NotificationError::UserNotFound(Faker.fake()),
                                )
                            })
                            .collect(),
                        processed: vec![],
                    }
                })
            });

        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .return_once(|_| Box::pin(async { Ok(mk_user(UserTier::Free)) }));

        let service = PeriodicMatcherServiceImpl::new(
            &filter_service,
            &query_service,
            &embedding_service,
            &enhanced_service,
            &notification_service,
            &user_service,
        );

        let result = service.process_filter(filter).await;

        assert!(matches!(
            result,
            Err(PeriodicMatcherError::UserSearchFilterError(
                UserSearchFilterError::SdkBatchWriteItemError(_)
            ))
        ));
    }
}
