use common::actor::domain::Actor;
use common::enhanced_match_reason::EnhancedMatchReason;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::language::domain::Language;
use common::product_lifecycle::domain::ProductLifecycle;
use notification::core::notification::{NotificationPayload, NotificationSearchFilterPayload};
use notification::service::command::CreateNotificationCommand;
use product::core::description::Description;
use product::core::product::Product;
use product::core::product_event::ProductEvent;
use product::core::product_event::domain::ProductDomainEventPayload;
use product::core::title::Title;
use product::opensearch::product_document::ProductDocument;
use product::service::get_service::{GetProductError, GetProductService};
use search_filter::core::quota::SearchFilterQuota;
use search_filter::core::search_filter_product_match::SearchFilterProductMatch;
use search_filter::core::user_search_filter::UserSearchFilter;
use search_filter::service::enhanced_search_match_service::EnhancedSearchMatchService;
use search_filter::service::user_search_filter_service::{
    UserSearchFilterError, UserSearchFilterService,
};
use std::collections::HashMap;
use time::OffsetDateTime;
use tracing::{debug, warn};
use user::service::user_service::{UserService, UserServiceError};

#[derive(Debug, thiserror::Error)]
pub enum ProductMatcherServiceError {
    #[error("GetProductError: {0}")]
    GetProductError(#[from] GetProductError),

    #[error("UserSearchFilterError: {0}")]
    UserSearchFilterError(#[from] UserSearchFilterError),

    #[error("UserServiceError: {0}")]
    UserServiceError(#[source] Box<UserServiceError>),
}

impl From<UserServiceError> for ProductMatcherServiceError {
    fn from(error: UserServiceError) -> Self {
        Self::UserServiceError(Box::new(error))
    }
}

/// An eligible match between a search filter and a product,
/// carrying the filter summary and optional enhanced match reason.
#[derive(Debug, Clone)]
pub struct EligibleMatch {
    pub filter: UserSearchFilter,
    pub enhanced_match_reason: Option<EnhancedMatchReason>,
}

/// The result of processing a product event: all eligible matches
/// and the subset of notification commands for quota-eligible users.
#[derive(Debug)]
pub struct ProductMatcherResult {
    pub matches: Vec<SearchFilterProductMatch>,
    pub notification_commands: Vec<CreateNotificationCommand>,
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait ProductMatcherService {
    async fn process_product_event(
        &self,
        event: ProductEvent,
    ) -> Result<ProductMatcherResult, ProductMatcherServiceError>;
}

pub struct ProductMatcherServiceImpl<'a> {
    user_search_filter_service: &'a (dyn UserSearchFilterService + Sync),
    get_product_service: &'a (dyn GetProductService + Sync),
    enhanced_search_match_service: &'a (dyn EnhancedSearchMatchService + Sync),
    user_service: &'a (dyn UserService + Sync),
}

impl<'a> ProductMatcherServiceImpl<'a> {
    pub fn new(
        user_search_filter_service: &'a (dyn UserSearchFilterService + Sync),
        get_product_service: &'a (dyn GetProductService + Sync),
        enhanced_search_match_service: &'a (dyn EnhancedSearchMatchService + Sync),
        user_service: &'a (dyn UserService + Sync),
    ) -> Self {
        Self {
            user_search_filter_service,
            get_product_service,
            enhanced_search_match_service,
            user_service,
        }
    }
}

/// Resolves the preferred language for a user, defaulting to English.
async fn resolve_user_language(
    user_service: &(dyn UserService + Sync),
    user_id: &common::user_id::UserId,
) -> Language {
    match user_service.find_user(user_id).await {
        Ok(user) => user.language.unwrap_or_default(),
        Err(err) => {
            warn!(
                userId = %user_id,
                error = %err,
                "Failed loading user for language resolution. Defaulting to English."
            );
            Language::default()
        }
    }
}

/// Resolves the product title, preferring the English translation.
/// Falls back to native title when English is unavailable.
fn product_title(product: &Product) -> Title {
    let titles = product.titles();
    titles
        .get(&Language::En)
        .cloned()
        .unwrap_or_else(|| product.native_title.payload.clone())
}

/// Resolves the product description, preferring the English translation.
/// Returns an empty description when none is available.
fn product_description(product: &Product) -> Description {
    let descriptions = product.descriptions();
    descriptions
        .get(&Language::En)
        .cloned()
        .unwrap_or_else(|| Description::from(""))
}

impl<'a> ProductMatcherServiceImpl<'a> {
    /// Determines eligible matches for a product event by:
    /// 1. Resolving the product from the event
    /// 2. Running percolation against stored search filters
    /// 3. Filtering out already-matched search filters
    /// 4. Running enhanced AI matching for filters with enhanced_search_description
    ///
    /// Returns the resolved product and a list of eligible matches.
    async fn determine_eligible_matches(
        &self,
        event: ProductEvent,
    ) -> Result<(EventId, Product, Vec<EligibleMatch>), ProductMatcherServiceError> {
        let event_id = event.event_id;
        let product_key = event.payload.key();
        let product = match event.payload {
            product::core::product_event::ProductEventPayload::ProductDomainEvent(
                ProductDomainEventPayload::Created(created_payload),
            ) => Product {
                product_id: event.aggregate_id,
                product_slug_id: created_payload.product_slug_id,
                shop_slug_id: created_payload.shop_slug_id,
                seller_slug_id: created_payload.seller_slug_id,
                event_id: event.event_id,
                shop_id: created_payload.shop_id,
                seller_id: created_payload.seller_id,
                shops_product_id: created_payload.shops_product_id,
                shop_name: created_payload.shop_name,
                seller_name: created_payload.seller_name,
                shop_type: created_payload.shop_type,
                structured_address: created_payload.structured_address,
                geo_address: created_payload.geo_address,
                native_title: created_payload.native_title,
                other_title: Default::default(),
                native_description: created_payload.native_description,
                native_price: created_payload.native_price,
                other_price: created_payload.other_price,
                native_price_estimate_min: created_payload.native_price_estimate_min,
                other_price_estimate_min: created_payload.other_price_estimate_min,
                native_price_estimate_max: created_payload.native_price_estimate_max,
                other_price_estimate_max: created_payload.other_price_estimate_max,
                state: created_payload.state,
                lifecycle: ProductLifecycle::Active,
                url: created_payload.url,
                view_url: created_payload.view_url,
                images: created_payload.images,
                embedding: None,
                auction_start: created_payload.auction_start,
                auction_end: created_payload.auction_end,
                created_by: Actor::System,
                updated_by: Actor::System,
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
        let matched_filters: Vec<_> = self
            .user_search_filter_service
            .match_user_search_filters(&product_document)
            .await?
            .into_iter()
            .filter(|filter| filter.state.is_active())
            .collect();

        if matched_filters.is_empty() {
            return Ok((event_id, product, vec![]));
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
            return Ok((event_id, product, vec![]));
        }

        // Run enhanced AI matching for filters with enhanced_search_description
        let title = product_title(&product);
        let description = product_description(&product);
        let images: Vec<_> = product.images.iter().take(5).cloned().collect();
        let mut eligible_matches = Vec::with_capacity(unmatched_filters.len());

        for filter in unmatched_filters {
            match &filter.search.enhanced_search_description {
                Some(enhanced_desc) => {
                    let language = resolve_user_language(self.user_service, &filter.user_id).await;
                    let eval_result = self
                        .enhanced_search_match_service
                        .evaluate(enhanced_desc, &title, &description, language, &images)
                        .await;

                    match eval_result {
                        Ok(result) if result.matches => {
                            debug!(
                                userId = %filter.user_id,
                                searchFilterId = %filter.user_search_filter_id,
                                "Enhanced search match confirmed for product."
                            );
                            eligible_matches.push(EligibleMatch {
                                filter,
                                enhanced_match_reason: result.reason,
                            });
                        }
                        Ok(_) => {
                            debug!(
                                userId = %filter.user_id,
                                searchFilterId = %filter.user_search_filter_id,
                                "Enhanced search match rejected product."
                            );
                        }
                        Err(err) => {
                            warn!(
                                userId = %filter.user_id,
                                searchFilterId = %filter.user_search_filter_id,
                                error = %err,
                                "Enhanced search match evaluation failed. Including filter without reason."
                            );
                            eligible_matches.push(EligibleMatch {
                                filter,
                                enhanced_match_reason: None,
                            });
                        }
                    }
                }
                None => {
                    eligible_matches.push(EligibleMatch {
                        filter,
                        enhanced_match_reason: None,
                    });
                }
            }
        }

        Ok((event_id, product, eligible_matches))
    }

    /// Determines notification commands from eligible matches by filtering
    /// out users who have exceeded their monthly search-filter-match quota.
    async fn determine_notification_commands(
        &self,
        eligible_matches: &[EligibleMatch],
        product: &Product,
    ) -> Result<Vec<CreateNotificationCommand>, ProductMatcherServiceError> {
        let mut commands = Vec::with_capacity(eligible_matches.len());
        let mut user_quota_cache: HashMap<common::user_id::UserId, bool> = HashMap::new();

        for eligible_match in eligible_matches {
            let filter = &eligible_match.filter;
            let eligible = match user_quota_cache.get(&filter.user_id) {
                Some(&cached) => cached,
                None => {
                    let eligible = match self.user_service.find_user(&filter.user_id).await {
                        Ok(user) => {
                            let match_count = self
                                .user_search_filter_service
                                .count_user_search_filter_matches_for_this_month(&filter.user_id)
                                .await?;
                            let quota = user.tier.search_filter_match_quota();
                            (match_count as u32) < quota
                        }
                        Err(err) => {
                            warn!(
                                userId = %filter.user_id,
                                error = %err,
                                "Failed loading user for quota check. Skipping notification."
                            );
                            false
                        }
                    };
                    user_quota_cache.insert(filter.user_id, eligible);
                    eligible
                }
            };

            if eligible {
                commands.push(mk_notification_command(product, filter));
            } else {
                debug!(
                    userId = %filter.user_id,
                    searchFilterId = %filter.user_search_filter_id,
                    "Skipping notification for user who exceeded search-filter-match quota."
                );
            }
        }

        Ok(commands)
    }
}

#[async_trait::async_trait]
impl<'a> ProductMatcherService for ProductMatcherServiceImpl<'a> {
    async fn process_product_event(
        &self,
        event: ProductEvent,
    ) -> Result<ProductMatcherResult, ProductMatcherServiceError> {
        let (event_id, product, eligible_matches) = self.determine_eligible_matches(event).await?;

        if eligible_matches.is_empty() {
            return Ok(ProductMatcherResult {
                matches: vec![],
                notification_commands: vec![],
            });
        }

        // Build SearchFilterProductMatch records for ALL eligible matches
        let now = OffsetDateTime::now_utc();
        let matches: Vec<SearchFilterProductMatch> = eligible_matches
            .iter()
            .map(|m| SearchFilterProductMatch {
                user_id: m.filter.user_id,
                user_search_filter_id: m.filter.user_search_filter_id,
                user_search_filter_name: Some(m.filter.name.clone()),
                shop_id: product.shop_id,
                shops_product_id: product.shops_product_id.clone(),
                product_id: product.product_id,
                origin_event_id: event_id,
                enhanced_match_reason: m.enhanced_match_reason.clone(),
                feedback: None,
                created_by: Actor::System,
                updated_by: Actor::System,
                created: now,
                updated: now,
            })
            .collect();

        // Determine notifications only for quota-eligible users
        let notification_commands = self
            .determine_notification_commands(&eligible_matches, &product)
            .await?;

        Ok(ProductMatcherResult {
            matches,
            notification_commands,
        })
    }
}

fn mk_notification_command(
    product: &Product,
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
            title: product.titles(),
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
    use common::event::Event;
    use common::event_id::EventId;
    use common::language::domain::Language;
    use common::product_state::domain::ProductState;
    use common::resource_state::domain::ResourceState;
    use common::user_id::UserId;
    use common::user_search_filter_id::UserSearchFilterId;
    use fake::{Fake, Faker};
    use product::core::product_event::ProductEventPayload;
    use product::core::product_event::domain::{
        ProductCreatedDomainEventPayload, ProductDomainEventPayload,
        ProductStateChangeDomainEventPayload,
    };
    use product::service::get_service::MockGetProductService;
    use search_filter::core::user_search_filter::EnhancedSearchDescription;
    use search_filter::core::user_search_filter::UserSearchFilter;
    use search_filter::core::user_search_filter_name::UserSearchFilterName;
    use search_filter::service::enhanced_search_match_service::{
        EnhancedSearchMatchError, EnhancedSearchMatchResult, MockEnhancedSearchMatchService,
    };
    use search_filter::service::user_search_filter_service::MockUserSearchFilterService;
    use serde::de::Error as _;
    use time::OffsetDateTime;
    use user::service::user_service::MockUserService;

    fn mk_event(product: &Product) -> ProductEvent {
        Event {
            aggregate_id: product.product_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductEventPayload::ProductDomainEvent(
                ProductDomainEventPayload::StateChanged(ProductStateChangeDomainEventPayload {
                    shop_id: product.shop_id,
                    seller_id: product.seller_id,
                    shops_product_id: product.shops_product_id.clone(),
                    old_state: ProductState::Available,
                    new_state: ProductState::Listed,
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
                    seller_slug_id: product.seller_slug_id.clone(),
                    shop_id: product.shop_id,
                    seller_id: product.shop_id,
                    shops_product_id: product.shops_product_id.clone(),
                    shop_name: product.shop_name.clone(),
                    seller_name: product.seller_name.clone(),
                    shop_type: product.shop_type,
                    structured_address: product.structured_address.clone(),
                    geo_address: product.geo_address,
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
                    view_url: product.view_url.clone(),
                    images: product.images.clone(),
                    auction_start: product.auction_start,
                    auction_end: product.auction_end,
                },
            )),
        }
    }

    fn mk_filter_summary_with_state(user_id: UserId, state: ResourceState) -> UserSearchFilter {
        UserSearchFilter {
            user_id,
            user_search_filter_id: UserSearchFilterId::new(),
            name: UserSearchFilterName::from("Test Filter"),
            search: Default::default(),
            notifications: true,
            state,
            created_by: Actor::System,
            updated_by: Actor::System,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
            last_hybrid_search_matched: OffsetDateTime::now_utc(),
            embedding: None,
        }
    }

    fn mk_filter_summary(user_id: UserId) -> UserSearchFilter {
        mk_filter_summary_with_state(user_id, ResourceState::Active)
    }

    fn mk_filter_summary_with_enhanced(user_id: UserId, description: &str) -> UserSearchFilter {
        UserSearchFilter {
            user_id,
            user_search_filter_id: UserSearchFilterId::new(),
            name: UserSearchFilterName::from("Enhanced Filter"),
            search: product::core::product_search::ProductSearch {
                enhanced_search_description: Some(EnhancedSearchDescription::from(description)),
                ..Default::default()
            },
            notifications: true,
            state: ResourceState::Active,
            created_by: Actor::System,
            updated_by: Actor::System,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
            last_hybrid_search_matched: OffsetDateTime::now_utc(),
            embedding: None,
        }
    }

    fn mk_default_enhanced_match_service() -> MockEnhancedSearchMatchService {
        MockEnhancedSearchMatchService::default()
    }

    fn mk_default_user_service() -> MockUserService {
        let mut user_service = MockUserService::default();
        user_service.expect_find_user().returning(|_| {
            Box::pin(async {
                let mut user: user::core::user::User = Faker.fake();
                user.tier = user::core::tier::UserTier::Pro;
                Ok(user)
            })
        });
        user_service
    }

    fn mk_user_with_language(language: Language) -> user::core::user::User {
        let mut user: user::core::user::User = Faker.fake();
        user.language = Some(language);
        user
    }

    #[tokio::test]
    async fn should_return_empty_when_no_filters_match() {
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

        let enhanced_service = mk_default_enhanced_match_service();
        let user_service = mk_default_user_service();

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        assert!(result.is_ok());
        let r = result.unwrap();
        assert!(r.matches.is_empty());
        assert!(r.notification_commands.is_empty());
    }

    #[tokio::test]
    async fn should_return_matches_and_commands_when_filters_match() {
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
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .returning(|_| Box::pin(async { Ok(0) }));

        let enhanced_service = mk_default_enhanced_match_service();
        let user_service = mk_default_user_service();

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        assert!(result.is_ok());
        let r = result.unwrap();
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].user_id, user_id);
        assert_eq!(r.notification_commands.len(), 1);
        assert_eq!(r.notification_commands[0].user_id, user_id);
        assert!(r.notification_commands[0].external);
    }

    #[tokio::test]
    async fn should_return_empty_when_only_inactive_filters_match() {
        let product: Product = Faker.fake();
        let event = mk_event(&product);
        let product_clone = product.clone();
        let summary = mk_filter_summary_with_state(UserId::new(), ResourceState::InactiveByUser);

        let mut get_service = MockGetProductService::default();
        get_service
            .expect_find_product()
            .return_once(move |_, _| Box::pin(async move { Ok(product_clone) }));

        let mut filter_service = MockUserSearchFilterService::default();
        filter_service
            .expect_match_user_search_filters()
            .return_once(move |_| Box::pin(async move { Ok(vec![summary]) }));

        let enhanced_service = mk_default_enhanced_match_service();
        let user_service = mk_default_user_service();

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        assert!(result.is_ok());
        let r = result.unwrap();
        assert!(r.matches.is_empty());
        assert!(r.notification_commands.is_empty());
    }

    #[tokio::test]
    async fn should_return_matches_and_commands_only_for_active_filters_when_active_and_inactive_filters_match()
     {
        let product: Product = Faker.fake();
        let event = mk_event(&product);
        let product_clone = product.clone();
        let active_user_id = UserId::new();
        let inactive_user_id = UserId::new();
        let active_summary = mk_filter_summary(active_user_id);
        let active_filter_id = active_summary.user_search_filter_id;
        let inactive_summary =
            mk_filter_summary_with_state(inactive_user_id, ResourceState::InactiveByRestrictedPlan);

        let mut get_service = MockGetProductService::default();
        get_service
            .expect_find_product()
            .return_once(move |_, _| Box::pin(async move { Ok(product_clone) }));

        let mut filter_service = MockUserSearchFilterService::default();
        filter_service
            .expect_match_user_search_filters()
            .return_once(move |_| {
                Box::pin(async move { Ok(vec![active_summary, inactive_summary]) })
            });
        filter_service
            .expect_find_search_filter_product_match()
            .withf(move |user_id, filter_id, _, _| {
                *user_id == active_user_id && *filter_id == active_filter_id
            })
            .return_once(|_, _, _, _| Box::pin(async { Ok(None) }));
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .withf(move |user_id| *user_id == active_user_id)
            .return_once(|_| Box::pin(async { Ok(0) }));

        let enhanced_service = mk_default_enhanced_match_service();
        let user_service = mk_default_user_service();

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        assert!(result.is_ok());
        let r = result.unwrap();
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].user_id, active_user_id);
        assert_eq!(r.notification_commands.len(), 1);
        assert_eq!(r.notification_commands[0].user_id, active_user_id);
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
        let enhanced_service = mk_default_enhanced_match_service();
        let user_service = mk_default_user_service();

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ProductMatcherServiceError::GetProductError(_)
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

        let enhanced_service = mk_default_enhanced_match_service();
        let user_service = mk_default_user_service();

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ProductMatcherServiceError::UserSearchFilterError(_)
        ));
    }

    #[tokio::test]
    async fn should_include_filter_id_and_name_when_creating_command() {
        let product: Product = Faker.fake();
        let event = mk_event(&product);
        let product_clone = product.clone();
        let filter_id = UserSearchFilterId::new();
        let filter_name = UserSearchFilterName::from("My Antiques Filter");
        let summary = UserSearchFilter {
            user_id: UserId::new(),
            user_search_filter_id: filter_id,
            name: filter_name.clone(),
            search: Default::default(),
            notifications: true,
            state: ResourceState::Active,
            created_by: Actor::System,
            updated_by: Actor::System,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
            last_hybrid_search_matched: OffsetDateTime::now_utc(),
            embedding: None,
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
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .returning(|_| Box::pin(async { Ok(0) }));

        let enhanced_service = mk_default_enhanced_match_service();
        let user_service = mk_default_user_service();

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        let r = result.unwrap();
        assert_eq!(r.notification_commands.len(), 1);
        match &r.notification_commands[0].notification_payload {
            NotificationPayload::SearchFilter {
                search_filter_payload,
                ..
            } => {
                assert_eq!(search_filter_payload.user_search_filter_id, filter_id);
                assert_eq!(search_filter_payload.user_search_filter_name, filter_name);
            }
            _ => panic!("Expected SearchFilter payload"),
        }
        // Also verify the match record has the filter id and name
        assert_eq!(r.matches[0].user_search_filter_id, filter_id);
        assert_eq!(r.matches[0].user_search_filter_name, Some(filter_name));
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
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .returning(|_| Box::pin(async { Ok(0) }));

        let enhanced_service = mk_default_enhanced_match_service();
        let user_service = mk_default_user_service();

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        let r = result.unwrap();
        match &r.notification_commands[0].notification_payload {
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
        assert_eq!(r.matches[0].product_id, expected_product_id);
        assert_eq!(r.matches[0].shop_id, expected_shop_id);
    }

    #[tokio::test]
    async fn should_filter_out_already_matched_filters() {
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
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .returning(|_| Box::pin(async { Ok(0) }));

        let enhanced_service = mk_default_enhanced_match_service();
        let user_service = mk_default_user_service();

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        assert!(result.is_ok());
        let r = result.unwrap();
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].user_id, user_id);
        assert_eq!(r.notification_commands.len(), 1);
        assert_eq!(r.notification_commands[0].user_id, user_id);
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

        let enhanced_service = mk_default_enhanced_match_service();
        let user_service = mk_default_user_service();

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        assert!(result.is_ok());
        let r = result.unwrap();
        assert!(r.matches.is_empty());
        assert!(r.notification_commands.is_empty());
    }

    #[tokio::test]
    async fn should_return_matches_and_commands_when_created_event() {
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
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .returning(|_| Box::pin(async { Ok(0) }));

        let enhanced_service = mk_default_enhanced_match_service();
        let user_service = mk_default_user_service();

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        assert!(result.is_ok());
        let r = result.unwrap();
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].user_id, user_id);
        assert_eq!(r.notification_commands.len(), 1);
        assert_eq!(r.notification_commands[0].user_id, user_id);
        assert!(r.notification_commands[0].external);
    }

    #[tokio::test]
    async fn should_return_empty_when_only_inactive_filters_match_for_created_events() {
        let product: Product = Faker.fake();
        let event = mk_created_event(&product);
        let summary =
            mk_filter_summary_with_state(UserId::new(), ResourceState::InactiveByRestrictedPlan);

        let get_service = MockGetProductService::default();

        let mut filter_service = MockUserSearchFilterService::default();
        filter_service
            .expect_match_user_search_filters()
            .return_once(move |_| Box::pin(async move { Ok(vec![summary]) }));

        let enhanced_service = mk_default_enhanced_match_service();
        let user_service = mk_default_user_service();

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        assert!(result.is_ok());
        let r = result.unwrap();
        assert!(r.matches.is_empty());
        assert!(r.notification_commands.is_empty());
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
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .returning(|_| Box::pin(async { Ok(0) }));

        let enhanced_service = mk_default_enhanced_match_service();
        let user_service = mk_default_user_service();

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        assert!(result.is_ok());
        let r = result.unwrap();
        assert_eq!(r.matches.len(), 1);
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
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .returning(|_| Box::pin(async { Ok(0) }));

        let enhanced_service = mk_default_enhanced_match_service();
        let user_service = mk_default_user_service();

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        let r = result.unwrap();
        assert_eq!(r.notification_commands.len(), 1);
        match &r.notification_commands[0].notification_payload {
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

        let enhanced_service = mk_default_enhanced_match_service();
        let user_service = mk_default_user_service();

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        assert!(result.is_ok());
        let r = result.unwrap();
        assert!(r.matches.is_empty());
        assert!(r.notification_commands.is_empty());
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
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .returning(|_| Box::pin(async { Ok(0) }));

        let enhanced_service = mk_default_enhanced_match_service();
        let user_service = mk_default_user_service();

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        assert!(result.is_ok());
        let r = result.unwrap();
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].user_id, user_id);
        assert_eq!(r.notification_commands.len(), 1);
        assert_eq!(r.notification_commands[0].user_id, user_id);
    }

    // ---------------------------------------------------------------------------
    // Image field is forwarded to notification command
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn should_set_first_image_when_product_has_images_for_search_filter_command() {
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
        let event = mk_event(&product);
        let product_clone = product.clone();
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
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .returning(|_| Box::pin(async { Ok(0) }));

        let enhanced_service = mk_default_enhanced_match_service();
        let user_service = mk_default_user_service();

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        let r = result.unwrap();
        assert_eq!(r.notification_commands.len(), 1);
        let actual_image = match &r.notification_commands[0].notification_payload {
            NotificationPayload::SearchFilter { image, .. } => image.clone(),
            _ => unreachable!("expected SearchFilter payload"),
        };
        assert_eq!(
            Some(expected_image),
            actual_image,
            "expected first image to be set"
        );
    }

    #[tokio::test]
    async fn should_set_image_to_none_when_product_has_no_images_for_search_filter_command() {
        let base: Product = Faker.fake();
        let product = Product {
            images: Default::default(),
            ..base
        };
        let event = mk_event(&product);
        let product_clone = product.clone();
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
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .returning(|_| Box::pin(async { Ok(0) }));

        let enhanced_service = mk_default_enhanced_match_service();
        let user_service = mk_default_user_service();

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        let r = result.unwrap();
        assert_eq!(r.notification_commands.len(), 1);
        let actual_image = match &r.notification_commands[0].notification_payload {
            NotificationPayload::SearchFilter { image, .. } => image.clone(),
            _ => unreachable!("expected SearchFilter payload"),
        };
        assert!(actual_image.is_none(), "expected image to be None");
    }

    // ---------------------------------------------------------------------------
    // Enhanced search match integration tests
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn should_include_reason_when_enhanced_match_confirms() {
        let product: Product = Faker.fake();
        let event = mk_event(&product);
        let product_clone = product.clone();
        let user_id = UserId::new();
        let summary = mk_filter_summary_with_enhanced(user_id, "Golden cufflinks with real ruby");

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
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .returning(|_| Box::pin(async { Ok(0) }));

        let mut enhanced_service = MockEnhancedSearchMatchService::default();
        enhanced_service
            .expect_evaluate()
            .return_once(|_, _, _, _, _| {
                Box::pin(async {
                    Ok(EnhancedSearchMatchResult {
                        matches: true,
                        reason: Some(EnhancedMatchReason::from("Matches golden cufflinks.")),
                    })
                })
            });

        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .returning(|_| Box::pin(async { Ok(mk_user_with_language(Language::En)) }));

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        let r = result.unwrap();
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].user_id, user_id);
        assert_eq!(
            r.matches[0].enhanced_match_reason,
            Some(EnhancedMatchReason::from("Matches golden cufflinks."))
        );
        assert_eq!(r.notification_commands.len(), 1);
        assert_eq!(r.notification_commands[0].user_id, user_id);
    }

    #[tokio::test]
    async fn should_exclude_filter_when_enhanced_match_rejects() {
        let product: Product = Faker.fake();
        let event = mk_event(&product);
        let product_clone = product.clone();
        let user_id = UserId::new();
        let summary = mk_filter_summary_with_enhanced(user_id, "Golden cufflinks with real ruby");

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

        let mut enhanced_service = MockEnhancedSearchMatchService::default();
        enhanced_service
            .expect_evaluate()
            .return_once(|_, _, _, _, _| {
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
            .returning(|_| Box::pin(async { Ok(mk_user_with_language(Language::En)) }));

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        let r = result.unwrap();
        assert!(r.matches.is_empty());
        assert!(r.notification_commands.is_empty());
    }

    #[tokio::test]
    async fn should_include_filter_without_reason_when_enhanced_match_errors() {
        let product: Product = Faker.fake();
        let event = mk_event(&product);
        let product_clone = product.clone();
        let user_id = UserId::new();
        let summary = mk_filter_summary_with_enhanced(user_id, "Golden cufflinks with real ruby");

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
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .returning(|_| Box::pin(async { Ok(0) }));

        let mut enhanced_service = MockEnhancedSearchMatchService::default();
        enhanced_service
            .expect_evaluate()
            .return_once(|_, _, _, _, _| {
                Box::pin(async {
                    Err(EnhancedSearchMatchError::InvalidResponse(
                        "test error".to_string(),
                    ))
                })
            });

        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .returning(|_| Box::pin(async { Ok(mk_user_with_language(Language::En)) }));

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        let r = result.unwrap();
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].user_id, user_id);
        assert!(r.matches[0].enhanced_match_reason.is_none());
        assert_eq!(r.notification_commands.len(), 1);
        assert_eq!(r.notification_commands[0].user_id, user_id);
    }

    #[tokio::test]
    async fn should_create_match_but_skip_notification_when_user_not_found_for_quota_check() {
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

        let enhanced_service = mk_default_enhanced_match_service();

        let mut user_service = MockUserService::default();
        user_service.expect_find_user().returning(|uid| {
            let uid = *uid;
            Box::pin(async move { Err(UserServiceError::UserNotFound(uid)) })
        });

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        let r = result.unwrap();
        // Match is still created
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].user_id, user_id);
        // But notification is skipped
        assert!(r.notification_commands.is_empty());
    }

    #[tokio::test]
    async fn should_mix_enhanced_and_plain_filters() {
        let product: Product = Faker.fake();
        let event = mk_event(&product);
        let product_clone = product.clone();
        let user1 = UserId::new();
        let user2 = UserId::new();
        let plain_summary = mk_filter_summary(user1);
        let enhanced_summary = mk_filter_summary_with_enhanced(user2, "Golden cufflinks");

        let mut get_service = MockGetProductService::default();
        get_service
            .expect_find_product()
            .return_once(move |_, _| Box::pin(async move { Ok(product_clone) }));

        let mut filter_service = MockUserSearchFilterService::default();
        filter_service
            .expect_match_user_search_filters()
            .return_once(move |_| {
                Box::pin(async move { Ok(vec![plain_summary, enhanced_summary]) })
            });
        filter_service
            .expect_find_search_filter_product_match()
            .times(2)
            .returning(|_, _, _, _| Box::pin(async { Ok(None) }));
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .returning(|_| Box::pin(async { Ok(0) }));

        let mut enhanced_service = MockEnhancedSearchMatchService::default();
        enhanced_service
            .expect_evaluate()
            .return_once(|_, _, _, _, _| {
                Box::pin(async {
                    Ok(EnhancedSearchMatchResult {
                        matches: true,
                        reason: Some(EnhancedMatchReason::from("Confirmed match.")),
                    })
                })
            });

        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .returning(|_| Box::pin(async { Ok(mk_user_with_language(Language::De)) }));

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        let r = result.unwrap();
        assert_eq!(r.matches.len(), 2);
        assert_eq!(r.notification_commands.len(), 2);

        // Plain filter has no reason on match
        let plain_match = r.matches.iter().find(|m| m.user_id == user1).unwrap();
        assert!(plain_match.enhanced_match_reason.is_none());

        // Enhanced filter has reason on match
        let enhanced_match = r.matches.iter().find(|m| m.user_id == user2).unwrap();
        assert_eq!(
            enhanced_match.enhanced_match_reason,
            Some(EnhancedMatchReason::from("Confirmed match."))
        );
    }

    // ---------------------------------------------------------------------------
    // Quota enforcement tests
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn should_create_match_but_skip_notification_when_user_exceeds_search_filter_match_quota()
    {
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
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .returning(|_| Box::pin(async { Ok(10) }));

        let enhanced_service = mk_default_enhanced_match_service();

        // Free tier user with quota of 10 — already at limit
        let mut user_service = MockUserService::default();
        user_service.expect_find_user().returning(|_| {
            Box::pin(async {
                let mut user: user::core::user::User = Faker.fake();
                user.tier = user::core::tier::UserTier::Free;
                Ok(user)
            })
        });

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        assert!(result.is_ok());
        let r = result.unwrap();
        // Match is created even though quota is exceeded
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].user_id, user_id);
        // But notification is NOT created
        assert!(r.notification_commands.is_empty());
    }

    #[tokio::test]
    async fn should_create_match_and_notification_when_user_below_search_filter_match_quota() {
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
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .returning(|_| Box::pin(async { Ok(9) }));

        let enhanced_service = mk_default_enhanced_match_service();

        // Free tier user with quota of 10 — 9 used, 1 remaining
        let mut user_service = MockUserService::default();
        user_service.expect_find_user().returning(|_| {
            Box::pin(async {
                let mut user: user::core::user::User = Faker.fake();
                user.tier = user::core::tier::UserTier::Free;
                Ok(user)
            })
        });

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        assert!(result.is_ok());
        let r = result.unwrap();
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].user_id, user_id);
        assert_eq!(r.notification_commands.len(), 1);
        assert_eq!(r.notification_commands[0].user_id, user_id);
    }

    #[tokio::test]
    async fn should_create_matches_for_both_but_skip_notification_for_quota_exceeded_user() {
        let product: Product = Faker.fake();
        let event = mk_event(&product);
        let product_clone = product.clone();

        let free_user_id = UserId::new();
        let pro_user_id = UserId::new();
        let free_summary = mk_filter_summary(free_user_id);
        let pro_summary = mk_filter_summary(pro_user_id);

        let mut get_service = MockGetProductService::default();
        get_service
            .expect_find_product()
            .return_once(move |_, _| Box::pin(async move { Ok(product_clone) }));

        let mut filter_service = MockUserSearchFilterService::default();
        filter_service
            .expect_match_user_search_filters()
            .return_once(move |_| Box::pin(async move { Ok(vec![free_summary, pro_summary]) }));
        filter_service
            .expect_find_search_filter_product_match()
            .times(2)
            .returning(|_, _, _, _| Box::pin(async { Ok(None) }));

        // Free user has 10 matches (at quota), Pro user has 100 matches (unlimited)
        let free_uid = free_user_id;
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .returning(move |uid| {
                let count = if *uid == free_uid { 10 } else { 100 };
                Box::pin(async move { Ok(count) })
            });

        let enhanced_service = mk_default_enhanced_match_service();

        let free_uid2 = free_user_id;
        let mut user_service = MockUserService::default();
        user_service.expect_find_user().returning(move |uid| {
            let tier = if *uid == free_uid2 {
                user::core::tier::UserTier::Free
            } else {
                user::core::tier::UserTier::Pro
            };
            Box::pin(async move {
                let mut user: user::core::user::User = Faker.fake();
                user.tier = tier;
                Ok(user)
            })
        });

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        assert!(result.is_ok());
        let r = result.unwrap();
        // Both users get matches
        assert_eq!(r.matches.len(), 2);
        assert!(r.matches.iter().any(|m| m.user_id == free_user_id));
        assert!(r.matches.iter().any(|m| m.user_id == pro_user_id));
        // Only pro user gets notification
        assert_eq!(r.notification_commands.len(), 1);
        assert_eq!(r.notification_commands[0].user_id, pro_user_id);
    }

    // ---------------------------------------------------------------------------
    // Image forwarding tests
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn should_forward_first_five_images_to_enhanced_match_service() {
        let images: Vec<product::core::product_image::ProductImage> =
            (0..7).map(|_| Faker.fake()).collect();
        let base: Product = Faker.fake();
        let product = Product {
            images: images.clone().into_iter().collect(),
            ..base
        };
        let event = mk_event(&product);
        let product_clone = product.clone();
        let user_id = UserId::new();
        let summary = mk_filter_summary_with_enhanced(user_id, "some description");

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
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .returning(|_| Box::pin(async { Ok(0) }));

        let expected_images: Vec<product::core::product_image::ProductImage> =
            images.iter().take(5).cloned().collect();

        let mut enhanced_service = MockEnhancedSearchMatchService::default();
        enhanced_service
            .expect_evaluate()
            .withf(move |_, _, _, _, imgs| imgs == expected_images.as_slice())
            .return_once(|_, _, _, _, _| {
                Box::pin(async {
                    Ok(EnhancedSearchMatchResult {
                        matches: true,
                        reason: None,
                    })
                })
            });

        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .returning(|_| Box::pin(async { Ok(mk_user_with_language(Language::En)) }));

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().matches.len(), 1);
    }

    #[tokio::test]
    async fn should_forward_empty_images_to_enhanced_match_service_when_product_has_no_images() {
        let base: Product = Faker.fake();
        let product = Product {
            images: Default::default(),
            ..base
        };
        let event = mk_event(&product);
        let product_clone = product.clone();
        let user_id = UserId::new();
        let summary = mk_filter_summary_with_enhanced(user_id, "some description");

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
        filter_service
            .expect_count_user_search_filter_matches_for_this_month()
            .returning(|_| Box::pin(async { Ok(0) }));

        let mut enhanced_service = MockEnhancedSearchMatchService::default();
        enhanced_service
            .expect_evaluate()
            .withf(|_, _, _, _, imgs| imgs.is_empty())
            .return_once(|_, _, _, _, _| {
                Box::pin(async {
                    Ok(EnhancedSearchMatchResult {
                        matches: true,
                        reason: None,
                    })
                })
            });

        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .returning(|_| Box::pin(async { Ok(mk_user_with_language(Language::En)) }));

        let service = ProductMatcherServiceImpl::new(
            &filter_service,
            &get_service,
            &enhanced_service,
            &user_service,
        );

        let result = service.process_product_event(event).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().matches.len(), 1);
    }
}
