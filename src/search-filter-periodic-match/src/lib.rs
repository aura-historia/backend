use common::actor::{RequestContext, domain::Actor};
use common::enhanced_match_reason::EnhancedMatchReason;
use common::language::domain::Language;
use common::pagination::cursor::Cursor;
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQueryTooShortError;
use common::resource_state::domain::ResourceState;
use futures_util::{StreamExt, stream};
use notification::core::notification::{NotificationPayload, NotificationSearchFilterPayload};
use notification::service::command::CreateNotificationCommand;
use notification::service::notification_service::{NotificationError, NotificationService};
use product::core::description::Description;
use product::core::product::LocalizedProductView;
use product::core::product_search::{EnhancedSearchDescription, ProductSearch};
use product::service::query_service::{QueryProductService, SearchProductsError};
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
use std::collections::HashSet;
use time::{Duration, OffsetDateTime};
use tracing::{debug, warn};
use user::service::user_service::{UserService, UserServiceError};

const FILTER_PAGE_SIZE: u64 = 100;
const HYBRID_PRODUCT_PAGE_SIZE: u64 = 50;
const MAX_ATTEMPTS: usize = 3;
pub const DEFAULT_LLM_CONCURRENCY: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeriodicMatcherResult {
    pub filters_processed: usize,
    pub matches_created: usize,
    pub notifications_created: usize,
    pub filters_failed: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum PeriodicMatcherError {
    #[error("UserSearchFilterError: {0}")]
    UserSearchFilterError(Box<UserSearchFilterError>),
    #[error("SearchProductsError: {0}")]
    SearchProductsError(#[from] SearchProductsError),
    #[error("Search filter '{0}' is missing its stored query embedding.")]
    MissingSearchFilterEmbedding(common::user_search_filter_id::UserSearchFilterId),
    #[error("EnhancedSearchMatchError: {0}")]
    EnhancedSearchMatchError(#[from] EnhancedSearchMatchError),
    #[error("UserServiceError: {0}")]
    UserServiceError(Box<UserServiceError>),
    #[error("NotificationError: {0}")]
    NotificationError(Box<NotificationError>),
    #[error("enhanced_search_description cannot be used as a product query: {0}")]
    InvalidEnhancedSearchDescription(#[from] TextQueryTooShortError<1>),
}

impl From<UserSearchFilterError> for PeriodicMatcherError {
    fn from(error: UserSearchFilterError) -> Self {
        Self::UserSearchFilterError(Box::new(error))
    }
}

impl From<UserServiceError> for PeriodicMatcherError {
    fn from(error: UserServiceError) -> Self {
        Self::UserServiceError(Box::new(error))
    }
}

impl From<NotificationError> for PeriodicMatcherError {
    fn from(error: NotificationError) -> Self {
        Self::NotificationError(Box::new(error))
    }
}

#[derive(Debug, Clone)]
struct AcceptedProductMatch {
    product: LocalizedProductView,
    enhanced_match_reason: Option<EnhancedMatchReason>,
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait PeriodicMatcherService {
    async fn match_active_filters(&self) -> Result<PeriodicMatcherResult, PeriodicMatcherError>;
}

pub struct PeriodicMatcherServiceImpl<'a> {
    user_search_filter_service: &'a (dyn UserSearchFilterService + Sync),
    query_product_service: &'a (dyn QueryProductService + Sync),
    enhanced_search_match_service: &'a (dyn EnhancedSearchMatchService + Sync + Send),
    notification_service: &'a (dyn NotificationService + Sync),
    user_service: &'a (dyn UserService + Sync),
    llm_concurrency: usize,
}

impl<'a> PeriodicMatcherServiceImpl<'a> {
    pub fn new(
        user_search_filter_service: &'a (dyn UserSearchFilterService + Sync),
        query_product_service: &'a (dyn QueryProductService + Sync),
        enhanced_search_match_service: &'a (dyn EnhancedSearchMatchService + Sync + Send),
        notification_service: &'a (dyn NotificationService + Sync),
        user_service: &'a (dyn UserService + Sync),
        llm_concurrency: usize,
    ) -> Self {
        Self {
            user_search_filter_service,
            query_product_service,
            enhanced_search_match_service,
            notification_service,
            user_service,
            llm_concurrency: llm_concurrency.max(1),
        }
    }

    async fn process_filter(
        &self,
        filter: UserSearchFilter,
    ) -> Result<(usize, usize), PeriodicMatcherError> {
        let Some(enhanced_description) = filter.search.enhanced_search_description.clone() else {
            debug!(
                userId = %filter.user_id,
                searchFilterId = %filter.user_search_filter_id,
                "Skipping periodic hybrid matching for filter without enhanced_search_description."
            );
            return Ok((0, 0));
        };
        if enhanced_description.as_ref().trim().is_empty() {
            debug!(
                userId = %filter.user_id,
                searchFilterId = %filter.user_search_filter_id,
                "Skipping periodic hybrid matching for filter with empty enhanced_search_description."
            );
            return Ok((0, 0));
        }

        let matched_at = OffsetDateTime::now_utc();
        let product_search = periodic_hybrid_search(
            &filter.search,
            &enhanced_description,
            filter.last_hybrid_search_matched,
        )?;
        let embedding = filter.embedding.as_deref().ok_or(
            PeriodicMatcherError::MissingSearchFilterEmbedding(filter.user_search_filter_id),
        )?;
        let cursor = Some(Cursor {
            size: HYBRID_PRODUCT_PAGE_SIZE,
            search_after: None,
        });

        let products = self
            .query_product_service
            .search_products_hybrid(&product_search, embedding, &cursor)
            .await?;

        debug!(
            userId = %filter.user_id,
            searchFilterId = %filter.user_search_filter_id,
            candidateCount = products.items.len(),
            "Retrieved periodic hybrid-search candidates."
        );

        let user = self.user_service.find_user(&filter.user_id).await?;
        let language = user.language.unwrap_or(Language::En);
        let accepted = self
            .evaluate_candidates(&filter, &enhanced_description, language, products.items)
            .await?;

        let matches_created = self.create_matches(&filter, accepted).await?;

        self.user_search_filter_service
            .update_user_search_filter(
                &RequestContext {
                    actor: Actor::System,
                },
                &filter.user_id,
                &filter.user_search_filter_id,
                UserSearchFilterUpdate {
                    updated: matched_at,
                    last_hybrid_search_matched: Some(matched_at),
                    ..Default::default()
                },
            )
            .await?;

        Ok(matches_created)
    }

    async fn evaluate_candidates(
        &self,
        filter: &UserSearchFilter,
        enhanced_description: &EnhancedSearchDescription,
        language: Language,
        products: Vec<LocalizedProductView>,
    ) -> Result<Vec<AcceptedProductMatch>, PeriodicMatcherError> {
        let evaluations = stream::iter(products)
            .map(|product| async move {
                self.evaluate_candidate(filter, enhanced_description, language, product)
                    .await
            })
            .buffer_unordered(self.llm_concurrency)
            .collect::<Vec<_>>()
            .await;

        let mut accepted = Vec::new();
        for evaluation in evaluations {
            if let Some(product_match) = evaluation? {
                accepted.push(product_match);
            }
        }
        Ok(accepted)
    }

    async fn evaluate_candidate(
        &self,
        filter: &UserSearchFilter,
        enhanced_description: &EnhancedSearchDescription,
        language: Language,
        product: LocalizedProductView,
    ) -> Result<Option<AcceptedProductMatch>, PeriodicMatcherError> {
        let description = product
            .description
            .as_ref()
            .map(|description| description.payload.clone())
            .unwrap_or_else(|| Description::from(""));
        let images: Vec<_> = product.images.iter().take(5).cloned().collect();
        let result = self
            .enhanced_search_match_service
            .evaluate(
                enhanced_description,
                &product.title.payload,
                &description,
                language,
                &images,
            )
            .await?;

        if result.matches {
            Ok(Some(AcceptedProductMatch {
                product,
                enhanced_match_reason: result.reason,
            }))
        } else {
            debug!(
                userId = %filter.user_id,
                searchFilterId = %filter.user_search_filter_id,
                "Enhanced matcher rejected periodic hybrid-search candidate."
            );
            Ok(None)
        }
    }

    async fn create_matches(
        &self,
        filter: &UserSearchFilter,
        accepted: Vec<AcceptedProductMatch>,
    ) -> Result<(usize, usize), PeriodicMatcherError> {
        if accepted.is_empty() {
            return Ok((0, 0));
        }

        let now = OffsetDateTime::now_utc();

        let mut matches = Vec::with_capacity(accepted.len());
        let mut matched_products = Vec::with_capacity(accepted.len());

        for accepted_match in accepted {
            let product = accepted_match.product;
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
                    "Skipping already-matched periodic hybrid-search candidate after LLM evaluation."
                );
                continue;
            }

            let match_item = SearchFilterProductMatch {
                user_id: filter.user_id,
                user_search_filter_id: filter.user_search_filter_id,
                user_search_filter_name: Some(filter.name.clone()),
                shop_id: product.shop_id,
                shops_product_id: product.shops_product_id.clone(),
                product_id: product.product_id,
                origin_event_id: product.event_id,
                enhanced_match_reason: accepted_match.enhanced_match_reason,
                feedback: None,
                created_by: Actor::System,
                updated_by: Actor::System,
                created: now,
                updated: now,
            };

            matches.push(match_item);
            matched_products.push(product);
        }

        if matches.is_empty() {
            return Ok((0, 0));
        }

        let match_count = self
            .user_search_filter_service
            .count_user_search_filter_matches_for_this_month(&filter.user_id)
            .await?;
        let user = self.user_service.find_user(&filter.user_id).await?;
        let mut remaining_quota = user
            .tier
            .search_filter_match_quota()
            .saturating_sub(match_count as u32);
        let mut notification_pairs = Vec::new();
        for product in &matched_products {
            if remaining_quota == 0 {
                break;
            }
            remaining_quota -= 1;
            notification_pairs.push((product.event_id, mk_notification_command(product, filter)));
        }

        let result = self
            .user_search_filter_service
            .create_search_filter_product_matches(
                &RequestContext {
                    actor: Actor::System,
                },
                matches,
            )
            .await?;
        if !result.unprocessed.is_empty() {
            return Err(UserSearchFilterError::PeriodicHybridMatchWriteIncomplete(
                result.unprocessed.len(),
            )
            .into());
        }

        let mut notifications_created = 0;
        for (origin_event_id, command) in notification_pairs {
            self.notification_service
                .create_notification(
                    &RequestContext {
                        actor: Actor::System,
                    },
                    &origin_event_id,
                    command,
                )
                .await?;
            notifications_created += 1;
        }

        Ok((result.processed.len(), notifications_created))
    }
}

#[async_trait::async_trait]
impl<'a> PeriodicMatcherService for PeriodicMatcherServiceImpl<'a> {
    async fn match_active_filters(&self) -> Result<PeriodicMatcherResult, PeriodicMatcherError> {
        let search = UserSearchFilterSearch {
            state: Some(ResourceState::Active),
            has_enhanced_search_description: Some(true),
        };
        let mut cursor = Some(Cursor {
            size: FILTER_PAGE_SIZE,
            search_after: None,
        });
        let mut result = PeriodicMatcherResult {
            filters_processed: 0,
            matches_created: 0,
            notifications_created: 0,
            filters_failed: 0,
        };
        let mut pending_retry: Vec<UserSearchFilter> = Vec::new();
        let mut seen_filters = HashSet::new();

        loop {
            let page = self
                .user_search_filter_service
                .search_user_search_filters(&search, &cursor)
                .await?;
            if page.items.is_empty() {
                break;
            }

            for filter in page.items {
                if !seen_filters.insert((filter.user_id, filter.user_search_filter_id)) {
                    debug!(
                        userId = %filter.user_id,
                        searchFilterId = %filter.user_search_filter_id,
                        "Skipping filter already processed in this periodic matching run."
                    );
                    continue;
                }

                match self.process_filter(filter.clone()).await {
                    Ok((matches_created, notifications_created)) => {
                        result.filters_processed += 1;
                        result.matches_created += matches_created;
                        result.notifications_created += notifications_created;
                    }
                    Err(err) => {
                        warn!(
                            userId = %filter.user_id,
                            searchFilterId = %filter.user_search_filter_id,
                            error = %err,
                            "Periodic filter processing failed; scheduled for retry."
                        );
                        pending_retry.push(filter);
                    }
                }
            }

            if page.cursor.search_after.is_none() || page.cursor.size == 0 {
                break;
            }
            cursor = Some(page.cursor);
        }

        for attempt in 2..=MAX_ATTEMPTS {
            if pending_retry.is_empty() {
                break;
            }
            let mut still_failing = Vec::new();
            for filter in pending_retry {
                match self.process_filter(filter.clone()).await {
                    Ok((matches_created, notifications_created)) => {
                        result.filters_processed += 1;
                        result.matches_created += matches_created;
                        result.notifications_created += notifications_created;
                    }
                    Err(err) => {
                        warn!(
                            userId = %filter.user_id,
                            searchFilterId = %filter.user_search_filter_id,
                            attempt,
                            error = %err,
                            "Periodic filter processing failed on retry."
                        );
                        still_failing.push(filter);
                    }
                }
            }
            pending_retry = still_failing;
        }

        result.filters_failed = pending_retry.len();
        Ok(result)
    }
}

fn periodic_hybrid_search(
    search: &ProductSearch,
    enhanced_description: &EnhancedSearchDescription,
    last_matched: OffsetDateTime,
) -> Result<ProductSearch, PeriodicMatcherError> {
    let mut search = search_since_last_match(search, last_matched);
    let enhanced_query = enhanced_description.as_ref().try_into()?;
    if !search
        .product_query
        .iter()
        .any(|query| query.as_ref() == enhanced_description.as_ref())
    {
        search.product_query.push(enhanced_query);
    }
    Ok(search)
}

fn search_since_last_match(search: &ProductSearch, last_matched: OffsetDateTime) -> ProductSearch {
    let mut search = search.clone();
    let min = last_matched + Duration::nanoseconds(1);
    search.updated_query = Some(match search.updated_query {
        Some(mut query) => {
            query.min = Some(query.min.map_or(min, |existing| existing.max(min)));
            query
        }
        None => RangeQuery {
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
    use common::currency::domain::Currency;
    use common::language::domain::Language;
    use common::localized::Localized;
    use common::pagination::cursor::CursoredResult;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use notification::core::notification::Notification;
    use notification::core::notification_id::NotificationId;
    use notification::service::notification_service::MockNotificationService;
    use product::core::product::LocalizedProductView;
    use product::core::title::Title;
    use product::service::query_service::MockQueryProductService;
    use search_filter::core::user_search_filter_name::UserSearchFilterName;
    use search_filter::service::enhanced_search_match_service::{
        EnhancedSearchMatchResult, MockEnhancedSearchMatchService,
    };
    use search_filter::service::user_search_filter_service::{
        CreateSearchFilterProductMatchesResult, MockUserSearchFilterService,
    };
    use serde_json::json;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use time::macros::datetime;
    use user::core::user::User;
    use user::service::user_service::{MockUserService, UserServiceError};

    fn mk_filter(enhanced_description: &str) -> UserSearchFilter {
        let mut filter: UserSearchFilter = Faker.fake();
        filter.name = UserSearchFilterName::from("Test Filter");
        filter.state = ResourceState::Active;
        filter.search = ProductSearch::new(Language::En, Currency::Eur);
        filter.search.enhanced_search_description =
            Some(EnhancedSearchDescription::from(enhanced_description));
        filter.last_hybrid_search_matched = datetime!(2026-05-10 12:00 UTC);
        filter.embedding = Some(vec![0.1, 0.2]);
        filter
    }

    fn mk_user(filter: &UserSearchFilter) -> User {
        let mut user: User = Faker.fake();
        user.user_id = filter.user_id;
        user.language = Some(Language::En);
        user
    }

    fn mk_product(title: &str) -> LocalizedProductView {
        let mut product: LocalizedProductView = Faker.fake();
        product.title = Localized::new(Language::En, Title::from(title));
        product
    }

    fn single_filter_page(
        filter: UserSearchFilter,
    ) -> CursoredResult<UserSearchFilter, serde_json::Value> {
        CursoredResult {
            items: vec![filter],
            cursor: Cursor {
                size: 1,
                search_after: None,
            },
            total: Some(1),
        }
    }

    fn expect_single_filter_scan(
        user_search_filter_service: &mut MockUserSearchFilterService,
        filter: UserSearchFilter,
    ) {
        user_search_filter_service
            .expect_search_user_search_filters()
            .times(1)
            .return_once(move |search, cursor| {
                assert_eq!(search.state, Some(ResourceState::Active));
                assert_eq!(search.has_enhanced_search_description, Some(true));
                let cursor = cursor
                    .as_ref()
                    .expect("periodic matcher should page filter scan");
                assert_eq!(cursor.size, FILTER_PAGE_SIZE);
                assert!(cursor.search_after.is_none());
                Box::pin(async move { Ok(single_filter_page(filter)) })
            });
    }

    fn expect_watermark_update(
        user_search_filter_service: &mut MockUserSearchFilterService,
        filter: UserSearchFilter,
    ) {
        user_search_filter_service
            .expect_update_user_search_filter()
            .times(1)
            .return_once(move |ctx, user_id, search_filter_id, update| {
                assert_eq!(ctx.actor, Actor::System);
                assert_eq!(*user_id, filter.user_id);
                assert_eq!(*search_filter_id, filter.user_search_filter_id);
                assert!(update.last_hybrid_search_matched.is_some());
                Box::pin(async move { Ok(filter) })
            });
    }

    #[tokio::test]
    async fn should_scan_active_enhanced_filters_and_request_top_50_hybrid_candidates() {
        let enhanced_description = "specific antique golden ring";
        let filter = mk_filter(enhanced_description);
        let last_matched = filter.last_hybrid_search_matched;
        let user = mk_user(&filter);

        let mut user_search_filter_service = MockUserSearchFilterService::default();
        expect_single_filter_scan(&mut user_search_filter_service, filter.clone());
        expect_watermark_update(&mut user_search_filter_service, filter.clone());

        let mut query_product_service = MockQueryProductService::default();
        query_product_service
            .expect_search_products_hybrid()
            .times(1)
            .return_once(move |search, embedding, cursor| {
                assert_eq!(search.product_query.len(), 1);
                assert_eq!(search.product_query[0].as_ref(), enhanced_description);
                assert_eq!(embedding, &[0.1, 0.2]);
                assert_eq!(
                    search.updated_query.as_ref().and_then(|query| query.min),
                    Some(last_matched + Duration::nanoseconds(1))
                );
                let cursor = cursor
                    .as_ref()
                    .expect("periodic matcher should request one hybrid product page");
                assert_eq!(cursor.size, HYBRID_PRODUCT_PAGE_SIZE);
                assert!(cursor.search_after.is_none());
                Box::pin(async {
                    Ok(CursoredResult {
                        items: vec![],
                        cursor: Cursor {
                            size: 0,
                            search_after: None,
                        },
                        total: None,
                    })
                })
            });

        let enhanced_search_match_service = MockEnhancedSearchMatchService::default();
        let notification_service = MockNotificationService::default();
        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .times(1)
            .return_once(move |_| Box::pin(async move { Ok(user) }));

        let matcher = PeriodicMatcherServiceImpl::new(
            &user_search_filter_service,
            &query_product_service,
            &enhanced_search_match_service,
            &notification_service,
            &user_service,
            DEFAULT_LLM_CONCURRENCY,
        );

        let result = matcher.match_active_filters().await.unwrap();

        assert_eq!(
            result,
            PeriodicMatcherResult {
                filters_processed: 1,
                matches_created: 0,
                notifications_created: 0,
                filters_failed: 0,
            }
        );
    }

    #[tokio::test]
    async fn should_create_match_and_notification_when_llm_accepts_candidate() {
        let enhanced_description = "matching antique vase";
        let filter = mk_filter(enhanced_description);
        let product = mk_product("accepted antique vase");
        let user = mk_user(&filter);
        let expected_reason = EnhancedMatchReason::from("It matches the requested antique vase.");

        let mut user_search_filter_service = MockUserSearchFilterService::default();
        expect_single_filter_scan(&mut user_search_filter_service, filter.clone());
        expect_watermark_update(&mut user_search_filter_service, filter.clone());
        let filter_for_existing_check = filter.clone();
        let product_for_existing_check = product.clone();
        user_search_filter_service
            .expect_find_search_filter_product_match()
            .times(1)
            .return_once(
                move |user_id, search_filter_id, shop_id, shops_product_id| {
                    assert_eq!(*user_id, filter_for_existing_check.user_id);
                    assert_eq!(
                        *search_filter_id,
                        filter_for_existing_check.user_search_filter_id
                    );
                    assert_eq!(*shop_id, product_for_existing_check.shop_id);
                    assert_eq!(
                        *shops_product_id,
                        product_for_existing_check.shops_product_id
                    );
                    Box::pin(async { Ok(None) })
                },
            );
        user_search_filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .times(1)
            .returning(|_| Box::pin(async { Ok(0) }));
        let product_for_match = product.clone();
        let expected_reason_for_match = expected_reason.clone();
        user_search_filter_service
            .expect_create_search_filter_product_matches()
            .times(1)
            .return_once(move |ctx, matches| {
                assert_eq!(ctx.actor, Actor::System);
                assert_eq!(matches.len(), 1);
                assert_eq!(matches[0].shop_id, product_for_match.shop_id);
                assert_eq!(
                    matches[0].shops_product_id,
                    product_for_match.shops_product_id
                );
                assert_eq!(matches[0].product_id, product_for_match.product_id);
                assert_eq!(
                    matches[0].enhanced_match_reason,
                    Some(expected_reason_for_match)
                );
                Box::pin(async move {
                    Ok(CreateSearchFilterProductMatchesResult {
                        processed: matches,
                        unprocessed: vec![],
                    })
                })
            });

        let product_for_search = product.clone();
        let mut query_product_service = MockQueryProductService::default();
        query_product_service
            .expect_search_products_hybrid()
            .times(1)
            .return_once(move |_, _, _| {
                Box::pin(async move {
                    Ok(CursoredResult {
                        items: vec![product_for_search],
                        cursor: Cursor {
                            size: 1,
                            search_after: None,
                        },
                        total: None,
                    })
                })
            });

        let mut enhanced_search_match_service = MockEnhancedSearchMatchService::default();
        let expected_reason_for_llm = expected_reason.clone();
        enhanced_search_match_service
            .expect_evaluate()
            .times(1)
            .return_once(move |description, title, _, language, _| {
                assert_eq!(description.as_ref(), enhanced_description);
                assert_eq!(title.as_ref(), "Accepted antique vase");
                assert_eq!(language, Language::En);
                Box::pin(async move {
                    Ok(EnhancedSearchMatchResult {
                        matches: true,
                        reason: Some(expected_reason_for_llm),
                    })
                })
            });

        let mut notification_service = MockNotificationService::default();
        let product_for_notification = product.clone();
        let filter_for_notification = filter.clone();
        notification_service
            .expect_create_notification()
            .times(1)
            .return_once(move |ctx, origin_event_id, command| {
                assert_eq!(ctx.actor, Actor::System);
                assert_eq!(*origin_event_id, product_for_notification.event_id);
                assert_eq!(command.user_id, filter_for_notification.user_id);
                let user_id = command.user_id;
                let external = command.external;
                let payload = command.notification_payload;
                match &payload {
                    NotificationPayload::SearchFilter {
                        product_id,
                        shop_id,
                        shops_product_id,
                        search_filter_payload,
                        ..
                    } => {
                        assert_eq!(*product_id, product_for_notification.product_id);
                        assert_eq!(*shop_id, product_for_notification.shop_id);
                        assert_eq!(*shops_product_id, product_for_notification.shops_product_id);
                        assert_eq!(
                            search_filter_payload.user_search_filter_id,
                            filter_for_notification.user_search_filter_id
                        );
                    }
                    payload => panic!("unexpected notification payload: {payload:?}"),
                }
                let origin_event_id = *origin_event_id;
                Box::pin(async move {
                    Ok(Notification {
                        user_id,
                        origin_event_id,
                        notification_id: NotificationId::new(),
                        notification_type: None,
                        notification_payload: payload,
                        seen: false,
                        external,
                        created_by: Actor::System,
                        updated_by: Actor::System,
                        created: OffsetDateTime::now_utc(),
                        updated: OffsetDateTime::now_utc(),
                    })
                })
            });

        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .times(2)
            .returning(move |_| {
                let user = user.clone();
                Box::pin(async move { Ok(user) })
            });

        let matcher = PeriodicMatcherServiceImpl::new(
            &user_search_filter_service,
            &query_product_service,
            &enhanced_search_match_service,
            &notification_service,
            &user_service,
            DEFAULT_LLM_CONCURRENCY,
        );

        let result = matcher.match_active_filters().await.unwrap();

        assert_eq!(result.matches_created, 1);
        assert_eq!(result.notifications_created, 1);
        assert_eq!(result.filters_failed, 0);
    }

    #[tokio::test]
    async fn should_evaluate_duplicate_candidate_but_not_persist_it_again() {
        let enhanced_description = "duplicate antique lamp";
        let filter = mk_filter(enhanced_description);
        let product = mk_product("duplicate antique lamp");
        let user = mk_user(&filter);

        let mut user_search_filter_service = MockUserSearchFilterService::default();
        expect_single_filter_scan(&mut user_search_filter_service, filter.clone());
        expect_watermark_update(&mut user_search_filter_service, filter.clone());
        let mut existing_match: SearchFilterProductMatch = Faker.fake();
        existing_match.user_id = filter.user_id;
        existing_match.user_search_filter_id = filter.user_search_filter_id;
        existing_match.shop_id = product.shop_id;
        existing_match.shops_product_id = product.shops_product_id.clone();
        user_search_filter_service
            .expect_find_search_filter_product_match()
            .times(1)
            .return_once(move |_, _, _, _| Box::pin(async move { Ok(Some(existing_match)) }));
        user_search_filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .never();
        user_search_filter_service
            .expect_create_search_filter_product_matches()
            .never();

        let product_for_search = product.clone();
        let mut query_product_service = MockQueryProductService::default();
        query_product_service
            .expect_search_products_hybrid()
            .times(1)
            .return_once(move |_, _, _| {
                Box::pin(async move {
                    Ok(CursoredResult {
                        items: vec![product_for_search],
                        cursor: Cursor {
                            size: 1,
                            search_after: None,
                        },
                        total: None,
                    })
                })
            });

        let mut enhanced_search_match_service = MockEnhancedSearchMatchService::default();
        enhanced_search_match_service
            .expect_evaluate()
            .times(1)
            .returning(|_, _, _, _, _| {
                Box::pin(async {
                    Ok(EnhancedSearchMatchResult {
                        matches: true,
                        reason: Some(EnhancedMatchReason::from("duplicate still evaluated")),
                    })
                })
            });

        let mut notification_service = MockNotificationService::default();
        notification_service.expect_create_notification().never();
        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .times(1)
            .return_once(move |_| Box::pin(async move { Ok(user) }));

        let matcher = PeriodicMatcherServiceImpl::new(
            &user_search_filter_service,
            &query_product_service,
            &enhanced_search_match_service,
            &notification_service,
            &user_service,
            DEFAULT_LLM_CONCURRENCY,
        );

        let result = matcher.match_active_filters().await.unwrap();

        assert_eq!(result.matches_created, 0);
        assert_eq!(result.notifications_created, 0);
        assert_eq!(result.filters_failed, 0);
    }

    #[tokio::test]
    async fn should_retry_failed_filter_and_report_failure_after_max_attempts() {
        let enhanced_description = "retry antique painting";
        let filter = mk_filter(enhanced_description);

        let mut user_search_filter_service = MockUserSearchFilterService::default();
        expect_single_filter_scan(&mut user_search_filter_service, filter.clone());
        user_search_filter_service
            .expect_update_user_search_filter()
            .never();

        let mut query_product_service = MockQueryProductService::default();
        query_product_service
            .expect_search_products_hybrid()
            .times(MAX_ATTEMPTS)
            .returning(|_, _, _| {
                Box::pin(async {
                    Ok(CursoredResult {
                        items: vec![],
                        cursor: Cursor {
                            size: 0,
                            search_after: None,
                        },
                        total: None,
                    })
                })
            });
        let enhanced_search_match_service = MockEnhancedSearchMatchService::default();
        let notification_service = MockNotificationService::default();
        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .times(MAX_ATTEMPTS)
            .returning(|_| Box::pin(async { Err(UserServiceError::UserNotFound(UserId::new())) }));

        let matcher = PeriodicMatcherServiceImpl::new(
            &user_search_filter_service,
            &query_product_service,
            &enhanced_search_match_service,
            &notification_service,
            &user_service,
            DEFAULT_LLM_CONCURRENCY,
        );

        let result = matcher.match_active_filters().await.unwrap();

        assert_eq!(result.filters_processed, 0);
        assert_eq!(result.filters_failed, 1);
    }

    #[tokio::test]
    async fn should_skip_filter_seen_again_after_watermark_update_changes_sort_position() {
        let enhanced_description = "resorted antique cabinet";
        let filter = mk_filter(enhanced_description);
        let user = mk_user(&filter);

        let first_page = CursoredResult {
            items: vec![filter.clone()],
            cursor: Cursor {
                size: 1,
                search_after: Some(json!([
                    "2026-05-10T12:00:00Z",
                    filter.user_search_filter_id
                ])),
            },
            total: Some(1),
        };
        let duplicate_page = single_filter_page(filter.clone());
        let pages = Arc::new(Mutex::new(VecDeque::from([first_page, duplicate_page])));

        let mut user_search_filter_service = MockUserSearchFilterService::default();
        user_search_filter_service
            .expect_search_user_search_filters()
            .times(2)
            .returning(move |_, _| {
                let page = pages.lock().unwrap().pop_front().unwrap();
                Box::pin(async move { Ok(page) })
            });
        expect_watermark_update(&mut user_search_filter_service, filter.clone());

        let mut query_product_service = MockQueryProductService::default();
        query_product_service
            .expect_search_products_hybrid()
            .times(1)
            .returning(|_, _, _| {
                Box::pin(async {
                    Ok(CursoredResult {
                        items: vec![],
                        cursor: Cursor {
                            size: 0,
                            search_after: None,
                        },
                        total: None,
                    })
                })
            });
        let enhanced_search_match_service = MockEnhancedSearchMatchService::default();
        let notification_service = MockNotificationService::default();
        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .times(1)
            .return_once(move |_| Box::pin(async move { Ok(user) }));

        let matcher = PeriodicMatcherServiceImpl::new(
            &user_search_filter_service,
            &query_product_service,
            &enhanced_search_match_service,
            &notification_service,
            &user_service,
            DEFAULT_LLM_CONCURRENCY,
        );

        let result = matcher.match_active_filters().await.unwrap();

        assert_eq!(result.filters_processed, 1);
        assert_eq!(result.filters_failed, 0);
    }

    #[test]
    fn should_add_updated_min_after_last_match_when_none_exists() {
        let search = ProductSearch::new(Language::En, Currency::Eur);
        let last_matched = datetime!(2026-05-10 12:00 UTC);

        let updated = search_since_last_match(&search, last_matched);

        assert_eq!(
            updated.updated_query,
            Some(RangeQuery {
                min: Some(last_matched + Duration::nanoseconds(1)),
                max: None,
            })
        );
        assert_eq!(search.updated_query, None);
    }

    #[test]
    fn should_keep_later_updated_min_when_search_already_has_one() {
        let existing_min = datetime!(2026-05-20 00:00 UTC);
        let last_matched = datetime!(2026-05-10 12:00 UTC);
        let search = ProductSearch {
            updated_query: Some(RangeQuery {
                min: Some(existing_min),
                max: Some(datetime!(2026-06-01 00:00 UTC)),
            }),
            ..ProductSearch::new(Language::En, Currency::Eur)
        };

        let updated = search_since_last_match(&search, last_matched);

        assert_eq!(updated.updated_query.unwrap().min, Some(existing_min));
    }

    #[test]
    fn should_add_enhanced_description_to_periodic_hybrid_query() {
        let search = ProductSearch::new(Language::En, Currency::Eur)
            .with_product_query("short bm25 query".try_into().unwrap());
        let enhanced = EnhancedSearchDescription::from("very specific antique gold ring");

        let periodic =
            periodic_hybrid_search(&search, &enhanced, datetime!(2026-05-10 12:00 UTC)).unwrap();

        assert_eq!(periodic.product_query.len(), 2);
        assert_eq!(periodic.product_query[0].as_ref(), "short bm25 query");
        assert_eq!(
            periodic.product_query[1].as_ref(),
            "very specific antique gold ring"
        );
    }
}
