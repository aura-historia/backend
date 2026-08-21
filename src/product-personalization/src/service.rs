use common::{
    api::error::ApiError, enhanced_match_reason::EnhancedMatchReason, language::domain::Language,
    localized::Localized, personalized::Personalized, product_id::ProductId,
    product_state::domain::ProductState, user_id::UserId,
    user_search_filter_name::UserSearchFilterName,
};
use notification::service::notification_service::{NotificationError, NotificationService};
use product::core::{
    product::LocalizedProductView,
    title::Title,
    user_state::{
        NotificationUserState, ProductUserState, ProhibitedContentUserState, SearchFilterUserState,
        WatchlistUserState,
    },
};
use product_watchlist::{
    dynamodb::repository::WatchlistProductDynamoDbRepository,
    service::product_watchlist_service::WatchProductError,
};
use search_filter::core::quota::SearchFilterQuota;
use search_filter::dynamodb::repository::UserSearchFilterDynamoDbRepository;
use std::collections::HashMap;
use time::OffsetDateTime;
use user::service::user_service::{UserService, UserServiceError};

#[derive(Debug, thiserror::Error)]
pub enum ProductPersonalizationError {
    #[error("WatchProductError: {0}")]
    WatchProductError(#[from] WatchProductError),
    #[error("UserServiceError: {0}")]
    UserServiceError(#[from] UserServiceError),
    #[error("NotificationError: {0}")]
    NotificationError(#[from] NotificationError),
    #[error("SearchFilterMatchError: {0}")]
    SearchFilterMatchError(String),
    #[error("UserSearchFilterError: {0}")]
    UserSearchFilterError(String),
}

impl From<ProductPersonalizationError> for ApiError {
    fn from(value: ProductPersonalizationError) -> Self {
        match value {
            ProductPersonalizationError::WatchProductError(e) => e.into(),
            ProductPersonalizationError::UserServiceError(_)
            | ProductPersonalizationError::NotificationError(_)
            | ProductPersonalizationError::SearchFilterMatchError(_)
            | ProductPersonalizationError::UserSearchFilterError(_) => {
                ApiError::internal_server_error(
                    common::api::error_code::INTERNAL_SERVER_ERROR,
                    Box::new(value),
                )
            }
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait ProductPersonalizationService {
    async fn personalize_watchlist(
        &self,
        user_id: &UserId,
        product: LocalizedProductView,
    ) -> Result<Personalized<LocalizedProductView, WatchlistUserState>, ProductPersonalizationError>;

    async fn personalize_all_watchlist(
        &self,
        user_id: &UserId,
        items: Vec<LocalizedProductView>,
    ) -> Result<
        Vec<Personalized<LocalizedProductView, WatchlistUserState>>,
        ProductPersonalizationError,
    >;

    async fn personalize_prohibited_content(
        &self,
        user_id: &UserId,
        product: LocalizedProductView,
    ) -> Result<
        Personalized<LocalizedProductView, ProhibitedContentUserState>,
        ProductPersonalizationError,
    >;

    async fn personalize_all_prohibited_content(
        &self,
        user_id: &UserId,
        products: Vec<LocalizedProductView>,
    ) -> Result<
        Vec<Personalized<LocalizedProductView, ProhibitedContentUserState>>,
        ProductPersonalizationError,
    >;

    async fn personalize_product_notification(
        &self,
        user_id: &UserId,
        product: LocalizedProductView,
    ) -> Result<
        Personalized<LocalizedProductView, NotificationUserState>,
        ProductPersonalizationError,
    >;

    async fn personalize_product_notification_all(
        &self,
        user_id: &UserId,
        products: Vec<LocalizedProductView>,
    ) -> Result<
        Vec<Personalized<LocalizedProductView, NotificationUserState>>,
        ProductPersonalizationError,
    >;

    async fn personalize_search_filter(
        &self,
        user_id: &UserId,
        product: LocalizedProductView,
    ) -> Result<
        Personalized<LocalizedProductView, SearchFilterUserState>,
        ProductPersonalizationError,
    >;

    async fn personalize_search_filter_all(
        &self,
        user_id: &UserId,
        products: Vec<LocalizedProductView>,
    ) -> Result<
        Vec<Personalized<LocalizedProductView, SearchFilterUserState>>,
        ProductPersonalizationError,
    >;

    async fn personalize(
        &self,
        user_id: &UserId,
        product: LocalizedProductView,
    ) -> Result<Personalized<LocalizedProductView, ProductUserState>, ProductPersonalizationError>;

    async fn personalize_all(
        &self,
        user_id: &UserId,
        products: Vec<LocalizedProductView>,
    ) -> Result<
        Vec<Personalized<LocalizedProductView, ProductUserState>>,
        ProductPersonalizationError,
    >;
}

pub struct ProductPersonalizationServiceImpl<'a> {
    watchlist_repository: &'a (dyn WatchlistProductDynamoDbRepository + Sync),
    notification_service: &'a (dyn NotificationService + Sync),
    user_service: &'a (dyn UserService + Sync),
    search_filter_repository: &'a (dyn UserSearchFilterDynamoDbRepository + Sync),
}

impl<'a> ProductPersonalizationServiceImpl<'a> {
    pub fn new(
        watchlist_repository: &'a (dyn WatchlistProductDynamoDbRepository + Sync),
        notification_service: &'a (dyn NotificationService + Sync),
        user_service: &'a (dyn UserService + Sync),
        search_filter_repository: &'a (dyn UserSearchFilterDynamoDbRepository + Sync),
    ) -> Self {
        Self {
            watchlist_repository,
            notification_service,
            user_service,
            search_filter_repository,
        }
    }

    #[allow(clippy::result_large_err)]
    async fn resolve_notification_state(
        &self,
        user_id: &UserId,
        product_id: &ProductId,
    ) -> Result<NotificationUserState, ProductPersonalizationError> {
        let notifications = self
            .notification_service
            .find_notifications_by_product(user_id, product_id, Some(1), false)
            .await?;

        let (seen, origin_event_id) = match notifications.first() {
            Some(n) => (n.seen, Some(n.origin_event_id)),
            None => (true, None),
        };
        Ok(NotificationUserState {
            seen,
            origin_event_id,
        })
    }

    #[allow(clippy::result_large_err)]
    async fn get_search_filter_match_quota(
        &self,
        user_id: &UserId,
    ) -> Result<Option<u32>, ProductPersonalizationError> {
        let user = self.user_service.find_user(user_id).await?;
        let quota = user.tier.search_filter_match_quota();
        if quota == u32::MAX {
            return Ok(None);
        }
        Ok(Some(quota))
    }

    #[allow(clippy::result_large_err)]
    async fn count_matches_up_to(
        &self,
        user_id: &UserId,
        created: &OffsetDateTime,
    ) -> Result<u64, ProductPersonalizationError> {
        let from = month_start(created);
        let count = self
            .search_filter_repository
            .count_user_search_filter_match_records_for_between(user_id, &from, created)
            .await
            .map_err(|e| ProductPersonalizationError::SearchFilterMatchError(e.to_string()))?;
        Ok(count)
    }
}

fn month_start(dt: &OffsetDateTime) -> OffsetDateTime {
    dt.replace_day(1)
        .expect("day 1 is always valid")
        .replace_hour(0)
        .expect("hour 0 is always valid")
        .replace_minute(0)
        .expect("minute 0 is always valid")
        .replace_second(0)
        .expect("second 0 is always valid")
        .replace_nanosecond(0)
        .expect("nanosecond 0 is always valid")
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

fn anonymize_product(product: &mut LocalizedProductView) {
    let nil = uuid::Uuid::nil();
    product.product_id = ProductId::from(nil);
    product.product_slug_id = common::product_slug_id::ProductSlugId::from("Hidden");
    product.shop_slug_id = common::shop_slug_id::ShopSlugId::from("Hidden");
    product.seller_slug_id = common::seller_slug_id::SellerSlugId::from("Hidden");
    product.event_id = common::event_id::EventId::from(nil);
    product.shop_id = common::shop_id::ShopId::from(nil);
    product.seller_id = common::shop_id::ShopId::from(nil);
    product.shops_product_id = common::shops_product_id::ShopsProductId::from(nil.to_string());
    product.shop_name = common::shop_name::ShopName::from("Hidden");
    product.seller_name = common::shop_name::ShopName::from("Hidden");
    let lang = product.title.localization;
    product.title = Localized::new(lang, hidden_title(lang));
    product.description = None;
    product.price = None;
    product.price_estimate_min = None;
    product.price_estimate_max = None;
    product.state = ProductState::Unknown;
    product.url = url::Url::parse("https://aura-historia.com/pricing").expect("valid url");
    product.images = Default::default();
    product.auction_start = None;
    product.auction_end = None;
    product.created = OffsetDateTime::UNIX_EPOCH;
    product.updated = OffsetDateTime::UNIX_EPOCH;
}

#[async_trait::async_trait]
impl<'a> ProductPersonalizationService for ProductPersonalizationServiceImpl<'a> {
    async fn personalize_watchlist(
        &self,
        user_id: &UserId,
        product: LocalizedProductView,
    ) -> Result<Personalized<LocalizedProductView, WatchlistUserState>, ProductPersonalizationError>
    {
        let watchlist_record = self
            .watchlist_repository
            .get_watchlist_record(user_id, &product.shop_id, &product.shops_product_id)
            .await
            .map_err(WatchProductError::from)?;

        let personalized = match watchlist_record {
            None => Personalized {
                item: product,
                user_state: Some(WatchlistUserState {
                    watching: false,
                    notifications: false,
                }),
            },
            Some(record) => Personalized {
                item: product,
                user_state: Some(WatchlistUserState {
                    watching: true,
                    notifications: record.notifications,
                }),
            },
        };
        Ok(personalized)
    }

    async fn personalize_all_watchlist(
        &self,
        user_id: &UserId,
        products: Vec<LocalizedProductView>,
    ) -> Result<
        Vec<Personalized<LocalizedProductView, WatchlistUserState>>,
        ProductPersonalizationError,
    > {
        let watchlist_records = self
            .watchlist_repository
            .query_watchlist_records_all(user_id, true)
            .await
            .map_err(WatchProductError::from)?
            .into_iter()
            .map(|watchlist_record| (watchlist_record.product_id, watchlist_record))
            .collect::<HashMap<_, _>>();

        let personalized_items = products
            .into_iter()
            .map(|item| {
                let user_state = watchlist_records
                    .get(&item.product_id)
                    .map(|watchlist_record| WatchlistUserState {
                        watching: true,
                        notifications: watchlist_record.notifications,
                    })
                    .unwrap_or(WatchlistUserState {
                        watching: false,
                        notifications: false,
                    });
                Personalized {
                    item,
                    user_state: Some(user_state),
                }
            })
            .collect();

        Ok(personalized_items)
    }

    async fn personalize_prohibited_content(
        &self,
        user_id: &UserId,
        product: LocalizedProductView,
    ) -> Result<
        Personalized<LocalizedProductView, ProhibitedContentUserState>,
        ProductPersonalizationError,
    > {
        let all_safe = product
            .images
            .iter()
            .all(|img| img.prohibited_content.is_safe());

        if all_safe {
            return Ok(Personalized {
                item: product,
                user_state: Some(ProhibitedContentUserState { consent: true }),
            });
        }

        let user = self.user_service.find_user(user_id).await?;
        Ok(Personalized {
            item: product,
            user_state: Some(ProhibitedContentUserState {
                consent: user.prohibited_content_consent,
            }),
        })
    }

    async fn personalize_all_prohibited_content(
        &self,
        user_id: &UserId,
        products: Vec<LocalizedProductView>,
    ) -> Result<
        Vec<Personalized<LocalizedProductView, ProhibitedContentUserState>>,
        ProductPersonalizationError,
    > {
        let all_safe = products.iter().all(|product| {
            product
                .images
                .iter()
                .all(|img| img.prohibited_content.is_safe())
        });

        let consent = if all_safe {
            true
        } else {
            let user = self.user_service.find_user(user_id).await?;
            user.prohibited_content_consent
        };

        let result = products
            .into_iter()
            .map(|product| Personalized {
                item: product,
                user_state: Some(ProhibitedContentUserState { consent }),
            })
            .collect();

        Ok(result)
    }

    async fn personalize_product_notification(
        &self,
        user_id: &UserId,
        product: LocalizedProductView,
    ) -> Result<
        Personalized<LocalizedProductView, NotificationUserState>,
        ProductPersonalizationError,
    > {
        let notification_state =
            Self::resolve_notification_state(self, user_id, &product.product_id).await?;
        Ok(Personalized {
            item: product,
            user_state: Some(notification_state),
        })
    }

    async fn personalize_product_notification_all(
        &self,
        user_id: &UserId,
        products: Vec<LocalizedProductView>,
    ) -> Result<
        Vec<Personalized<LocalizedProductView, NotificationUserState>>,
        ProductPersonalizationError,
    > {
        let futures: Vec<_> = products
            .iter()
            .map(|p| self.resolve_notification_state(user_id, &p.product_id))
            .collect();
        let results = futures::future::join_all(futures).await;

        let mut notification_states = HashMap::new();
        for (product, result) in products.iter().zip(results) {
            notification_states.insert(product.product_id, result?);
        }

        let result = products
            .into_iter()
            .map(|product| {
                let notification_state = notification_states
                    .get(&product.product_id)
                    .copied()
                    .unwrap_or_default();
                Personalized {
                    item: product,
                    user_state: Some(notification_state),
                }
            })
            .collect();

        Ok(result)
    }

    async fn personalize_search_filter(
        &self,
        user_id: &UserId,
        product: LocalizedProductView,
    ) -> Result<
        Personalized<LocalizedProductView, SearchFilterUserState>,
        ProductPersonalizationError,
    > {
        let match_records = self
            .search_filter_repository
            .query_user_search_filter_match_records_for_product(
                user_id,
                &product.shop_id,
                &product.shops_product_id,
            )
            .await
            .map_err(|e| ProductPersonalizationError::SearchFilterMatchError(e.to_string()))?;

        let (search_filter_state, product) = match match_records.first() {
            Some(record) => {
                let hidden = match self.get_search_filter_match_quota(user_id).await? {
                    Some(quota) => {
                        let position = self.count_matches_up_to(user_id, &record.created).await?;
                        position > quota as u64
                    }
                    None => false,
                };

                let mut product = product;
                if hidden {
                    anonymize_product(&mut product);
                }

                (
                    SearchFilterUserState {
                        matched: true,
                        hidden,
                        user_search_filter_id: Some(record.user_search_filter_id),
                        user_search_filter_name: record
                            .user_search_filter_name
                            .as_deref()
                            .map(UserSearchFilterName::from),
                        match_reason: record
                            .enhanced_match_reason
                            .as_deref()
                            .map(EnhancedMatchReason::from),
                        match_feedback: record.feedback,
                    },
                    product,
                )
            }
            None => (SearchFilterUserState::default(), product),
        };

        Ok(Personalized {
            item: product,
            user_state: Some(search_filter_state),
        })
    }

    async fn personalize_search_filter_all(
        &self,
        user_id: &UserId,
        products: Vec<LocalizedProductView>,
    ) -> Result<
        Vec<Personalized<LocalizedProductView, SearchFilterUserState>>,
        ProductPersonalizationError,
    > {
        let all_match_records = self
            .search_filter_repository
            .query_user_search_filter_match_records_all(user_id)
            .await
            .map_err(|e| ProductPersonalizationError::SearchFilterMatchError(e.to_string()))?;

        let match_by_product: HashMap<_, _> = all_match_records
            .into_iter()
            .map(|record| (record.product_id, record))
            .collect();

        let hidden_product_ids: std::collections::HashSet<ProductId> =
            if match_by_product.is_empty() {
                std::collections::HashSet::new()
            } else {
                match self.get_search_filter_match_quota(user_id).await? {
                    Some(quota) => {
                        let mut matches: Vec<_> = match_by_product.values().collect();
                        matches.sort_by_key(|record| record.created);

                        let mut hidden_product_ids = std::collections::HashSet::new();
                        let mut current_month = None;
                        let mut matches_in_month = 0usize;

                        for record in matches {
                            let record_month = month_start(&record.created);
                            if current_month != Some(record_month) {
                                current_month = Some(record_month);
                                matches_in_month = 0;
                            }

                            matches_in_month += 1;
                            if matches_in_month > quota as usize {
                                hidden_product_ids.insert(record.product_id);
                            }
                        }

                        hidden_product_ids
                    }
                    None => std::collections::HashSet::new(),
                }
            };

        let result = products
            .into_iter()
            .map(|mut product| {
                let search_filter_state = match match_by_product.get(&product.product_id) {
                    Some(record) => {
                        let hidden = hidden_product_ids.contains(&product.product_id);
                        if hidden {
                            anonymize_product(&mut product);
                        }
                        SearchFilterUserState {
                            matched: true,
                            hidden,
                            user_search_filter_id: Some(record.user_search_filter_id),
                            user_search_filter_name: record
                                .user_search_filter_name
                                .as_deref()
                                .map(UserSearchFilterName::from),
                            match_reason: record
                                .enhanced_match_reason
                                .as_deref()
                                .map(EnhancedMatchReason::from),
                            match_feedback: record.feedback,
                        }
                    }
                    None => SearchFilterUserState::default(),
                };
                Personalized {
                    item: product,
                    user_state: Some(search_filter_state),
                }
            })
            .collect();

        Ok(result)
    }

    // NOTE: Search-filter personalization MUST happen last as it may destructively
    // modify the `LocalizedProductView` (anonymize products when search-filter-match
    // quota is exceeded).
    async fn personalize(
        &self,
        user_id: &UserId,
        product: LocalizedProductView,
    ) -> Result<Personalized<LocalizedProductView, ProductUserState>, ProductPersonalizationError>
    {
        let watchlist = self.personalize_watchlist(user_id, product).await?;
        let prohibited_content = self
            .personalize_prohibited_content(user_id, watchlist.item)
            .await?;
        let notification = self
            .personalize_product_notification(user_id, prohibited_content.item)
            .await?;
        // Search-filter personalization MUST happen last as it may destructively
        // modify the LocalizedProductView when the search-filter-match quota is exceeded.
        let search_filter = self
            .personalize_search_filter(user_id, notification.item)
            .await?;
        Ok(Personalized {
            item: search_filter.item,
            user_state: Some(ProductUserState {
                watchlist: watchlist.user_state.unwrap_or_default(),
                prohibited_content: prohibited_content.user_state.unwrap_or_default(),
                notification: notification.user_state.unwrap_or_default(),
                search_filter: search_filter.user_state.unwrap_or_default(),
            }),
        })
    }

    async fn personalize_all(
        &self,
        user_id: &UserId,
        products: Vec<LocalizedProductView>,
    ) -> Result<
        Vec<Personalized<LocalizedProductView, ProductUserState>>,
        ProductPersonalizationError,
    > {
        let watchlist_results = self.personalize_all_watchlist(user_id, products).await?;

        let all_safe = watchlist_results.iter().all(|p| {
            p.item
                .images
                .iter()
                .all(|img| img.prohibited_content.is_safe())
        });

        let consent = if all_safe {
            true
        } else {
            let user = self.user_service.find_user(user_id).await?;
            user.prohibited_content_consent
        };

        let items_with_watchlist: Vec<_> = watchlist_results
            .into_iter()
            .map(|p| (p.item, p.user_state.unwrap_or_default()))
            .collect();

        let products_for_notification: Vec<LocalizedProductView> = items_with_watchlist
            .iter()
            .map(|(item, _)| item.clone())
            .collect();
        let notification_results = self
            .personalize_product_notification_all(user_id, products_for_notification)
            .await?;

        let products_for_search_filter: Vec<LocalizedProductView> = items_with_watchlist
            .iter()
            .map(|(item, _)| item.clone())
            .collect();
        // Search-filter personalization MUST happen last as it may destructively
        // modify the LocalizedProductView when the search-filter-match quota is exceeded.
        let search_filter_results = self
            .personalize_search_filter_all(user_id, products_for_search_filter)
            .await?;

        let result = items_with_watchlist
            .into_iter()
            .zip(notification_results)
            .zip(search_filter_results)
            .map(|(((_, watchlist), notif), sf)| Personalized {
                item: sf.item,
                user_state: Some(ProductUserState {
                    watchlist,
                    prohibited_content: ProhibitedContentUserState { consent },
                    notification: notif.user_state.unwrap_or_default(),
                    search_filter: sf.user_state.unwrap_or_default(),
                }),
            })
            .collect();

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use common::product_id::ProductId;
    use common::{actor::domain::Actor, actor::record::ActorRecord};
    use fake::{Fake, Faker};
    use product::core::product::LocalizedProductView;
    use product::core::product_image::ProductImage;
    use product::core::prohibited_content::ProhibitedContent;
    use product_watchlist::dynamodb::{
        record::WatchlistProductRecord, repository::MockWatchlistProductDynamoDbRepository,
    };
    use search_filter::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
    use time::OffsetDateTime;
    use user::core::user::User;
    use user::service::user_service::MockUserService;

    use crate::service::{
        ProductPersonalizationService, ProductPersonalizationServiceImpl, month_start,
    };
    use notification::service::notification_service::MockNotificationService;

    #[tokio::test]
    async fn should_personalize_watchlist_when_watching_notifications_false() {
        let product_id = ProductId::new();
        let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        watchlist_repository
            .expect_get_watchlist_record()
            .return_once(move |user_id, _, _| {
                let user_id = *user_id;
                Box::pin(async move {
                    let watched = WatchlistProductRecord {
                        shop_id: Faker.fake(),
                        shops_product_id: Faker.fake(),
                        product_id,
                        notifications: false,
                        state: common::resource_state::record::ResourceStateRecord::Active,
                        created_by: ActorRecord::System,
                        updated_by: ActorRecord::System,
                        created: OffsetDateTime::now_utc(),
                        updated: OffsetDateTime::now_utc(),
                        pk: "dummy".to_owned(),
                        sk: "dummy".to_owned(),
                        lsi1_sk: "dummy".to_owned(),
                        gsi1_pk: "dummy".to_owned(),
                        gsi1_sk: "dummy".to_owned(),
                        user_id,
                    };
                    Ok(Some(watched))
                })
            });

        let user_service = MockUserService::default();
        let notification_service = MockNotificationService::default();
        let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let mut input = Faker.fake::<LocalizedProductView>();
        input.product_id = product_id;

        let actual = service
            .personalize_watchlist(&Faker.fake(), input)
            .await
            .unwrap();

        assert!(actual.user_state.unwrap().watching);
        assert!(!actual.user_state.unwrap().notifications);
    }

    #[tokio::test]
    async fn should_personalize_watchlist_when_watching_notifications_true() {
        let product_id = ProductId::new();
        let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        watchlist_repository
            .expect_get_watchlist_record()
            .return_once(move |user_id, _, _| {
                let user_id = *user_id;
                Box::pin(async move {
                    let watched = WatchlistProductRecord {
                        shop_id: Faker.fake(),
                        shops_product_id: Faker.fake(),
                        product_id,
                        notifications: true,
                        state: common::resource_state::record::ResourceStateRecord::Active,
                        created_by: ActorRecord::System,
                        updated_by: ActorRecord::System,
                        created: OffsetDateTime::now_utc(),
                        updated: OffsetDateTime::now_utc(),
                        pk: "dummy".to_owned(),
                        sk: "dummy".to_owned(),
                        lsi1_sk: "dummy".to_owned(),
                        gsi1_pk: "dummy".to_owned(),
                        gsi1_sk: "dummy".to_owned(),
                        user_id,
                    };
                    Ok(Some(watched))
                })
            });

        let user_service = MockUserService::default();
        let notification_service = MockNotificationService::default();
        let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let mut input = Faker.fake::<LocalizedProductView>();
        input.product_id = product_id;

        let actual = service
            .personalize_watchlist(&Faker.fake(), input)
            .await
            .unwrap();

        assert!(actual.user_state.unwrap().watching);
        assert!(actual.user_state.unwrap().notifications);
    }

    #[tokio::test]
    async fn should_personalize_watchlist_when_not_watching() {
        let product_id = ProductId::new();
        let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        watchlist_repository
            .expect_get_watchlist_record()
            .return_once(move |_, _, _| Box::pin(async move { Ok(None) }));

        let user_service = MockUserService::default();
        let notification_service = MockNotificationService::default();
        let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let mut input = Faker.fake::<LocalizedProductView>();
        input.product_id = product_id;

        let actual = service
            .personalize_watchlist(&Faker.fake(), input)
            .await
            .unwrap();

        assert!(!actual.user_state.unwrap().watching);
        assert!(!actual.user_state.unwrap().notifications);
    }

    #[tokio::test]
    async fn should_personalize_watchlist_all() {
        let product1_id = ProductId::new();
        let product2_id = ProductId::new();
        let product3_id = ProductId::new();
        let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        watchlist_repository
            .expect_query_watchlist_records_all()
            .return_once(move |user_id, _| {
                let user_id = *user_id;
                Box::pin(async move {
                    let watched = vec![
                        WatchlistProductRecord {
                            shop_id: Faker.fake(),
                            shops_product_id: Faker.fake(),
                            product_id: product1_id,
                            notifications: false,
                            state: common::resource_state::record::ResourceStateRecord::Active,
                            created_by: ActorRecord::System,
                            updated_by: ActorRecord::System,
                            created: OffsetDateTime::now_utc(),
                            updated: OffsetDateTime::now_utc(),
                            pk: "dummy".to_owned(),
                            sk: "dummy".to_owned(),
                            lsi1_sk: "dummy".to_owned(),
                            gsi1_pk: "dummy".to_owned(),
                            gsi1_sk: "dummy".to_owned(),
                            user_id,
                        },
                        WatchlistProductRecord {
                            shop_id: Faker.fake(),
                            shops_product_id: Faker.fake(),
                            product_id: product2_id,
                            notifications: true,
                            state: common::resource_state::record::ResourceStateRecord::Active,
                            created_by: ActorRecord::System,
                            updated_by: ActorRecord::System,
                            created: OffsetDateTime::now_utc(),
                            updated: OffsetDateTime::now_utc(),
                            pk: "dummy".to_owned(),
                            sk: "dummy".to_owned(),
                            lsi1_sk: "dummy".to_owned(),
                            gsi1_pk: "dummy".to_owned(),
                            gsi1_sk: "dummy".to_owned(),
                            user_id,
                        },
                        WatchlistProductRecord {
                            shop_id: Faker.fake(),
                            shops_product_id: Faker.fake(),
                            product_id: product3_id,
                            notifications: true,
                            state: common::resource_state::record::ResourceStateRecord::Active,
                            created_by: ActorRecord::System,
                            updated_by: ActorRecord::System,
                            created: OffsetDateTime::now_utc(),
                            updated: OffsetDateTime::now_utc(),
                            pk: "dummy".to_owned(),
                            sk: "dummy".to_owned(),
                            lsi1_sk: "dummy".to_owned(),
                            gsi1_pk: "dummy".to_owned(),
                            gsi1_sk: "dummy".to_owned(),
                            user_id,
                        },
                    ];
                    Ok(watched)
                })
            });

        let user_service = MockUserService::default();
        let notification_service = MockNotificationService::default();
        let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let mut watched_in1 = Faker.fake::<LocalizedProductView>();
        watched_in1.product_id = product1_id;
        let mut watched_in2 = Faker.fake::<LocalizedProductView>();
        watched_in2.product_id = product2_id;
        let mut watched_in3 = Faker.fake::<LocalizedProductView>();
        watched_in3.product_id = product3_id;

        let input = vec![
            watched_in1,
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            watched_in2,
            Faker.fake(),
            watched_in3,
        ];
        let actual = service
            .personalize_all_watchlist(&Faker.fake(), input)
            .await
            .unwrap();

        assert!(
            !actual[0].user_state.unwrap().notifications && actual[0].user_state.unwrap().watching
        );
        assert!(
            !actual[1].user_state.unwrap().notifications && !actual[1].user_state.unwrap().watching
        );
        assert!(
            !actual[2].user_state.unwrap().notifications && !actual[2].user_state.unwrap().watching
        );
        assert!(
            !actual[3].user_state.unwrap().notifications && !actual[3].user_state.unwrap().watching
        );
        assert!(
            actual[4].user_state.unwrap().notifications && actual[4].user_state.unwrap().watching
        );
        assert!(
            !actual[5].user_state.unwrap().notifications && !actual[5].user_state.unwrap().watching
        );
        assert!(
            actual[6].user_state.unwrap().notifications && actual[6].user_state.unwrap().watching
        );
    }

    // ---- Prohibited content tests ----

    fn make_safe_image() -> ProductImage {
        let mut img = Faker.fake::<ProductImage>();
        img.prohibited_content = ProhibitedContent::None;
        img
    }

    fn make_unsafe_image() -> ProductImage {
        let mut img = Faker.fake::<ProductImage>();
        img.prohibited_content = ProhibitedContent::NaziGermany;
        img
    }

    fn make_unknown_image() -> ProductImage {
        let mut img = Faker.fake::<ProductImage>();
        img.prohibited_content = ProhibitedContent::Unknown;
        img
    }

    fn make_test_user(prohibited_content_consent: bool) -> User {
        User {
            user_id: Faker.fake(),
            email: "test@test.com".try_into().unwrap(),
            first_name: None,
            last_name: None,
            language: None,
            currency: None,
            measurement_unit: None,
            prohibited_content_consent,
            tier: user::core::tier::UserTier::Free,
            role: user::core::role::UserRole::User,
            stripe_customer_id: None,
            structured_address: None,
            geo_address: None,
            partner_shops: Default::default(),
            created_by: Actor::System,
            updated_by: Actor::System,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        }
    }

    fn make_test_notification(seen: bool) -> notification::core::notification::Notification {
        use notification::core::{
            notification::{Notification, NotificationPayload, NotificationWatchlistPayload},
            notification_id::NotificationId,
        };
        Notification {
            user_id: Faker.fake(),
            origin_event_id: Faker.fake(),
            notification_id: NotificationId::new(),
            notification_type: None,
            notification_payload: NotificationPayload::Watchlist {
                product_id: Faker.fake(),
                shop_id: Faker.fake(),
                shops_product_id: Faker.fake(),
                shop_slug_id: Faker.fake(),
                product_slug_id: Faker.fake(),
                shop_name: Faker.fake(),
                title: std::collections::HashMap::new(),
                image: None,
                url: url::Url::parse("https://example.com/item/1").unwrap(),
                view_url: url::Url::parse("https://example.com/item/1?utm_source=aura_historia")
                    .unwrap(),
                watchlist_payload: NotificationWatchlistPayload::StateChange {
                    old_state: common::product_state::domain::ProductState::Listed,
                    new_state: common::product_state::domain::ProductState::Available,
                },
            },
            seen,
            external: false,
            created_by: Actor::System,
            updated_by: Actor::System,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        }
    }

    #[tokio::test]
    async fn should_personalize_prohibited_content_consent_true_when_all_images_safe() {
        let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        let mut user_service = MockUserService::default();
        user_service.expect_find_user().never();

        let notification_service = MockNotificationService::default();
        let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let mut input = Faker.fake::<LocalizedProductView>();
        input.images = vec![make_safe_image(), make_safe_image()]
            .into_iter()
            .collect();

        let actual = service
            .personalize_prohibited_content(&Faker.fake(), input)
            .await
            .unwrap();

        assert!(actual.user_state.unwrap().consent);
    }

    #[tokio::test]
    async fn should_personalize_prohibited_content_consent_true_when_no_images() {
        let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        let mut user_service = MockUserService::default();
        user_service.expect_find_user().never();

        let notification_service = MockNotificationService::default();
        let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let mut input = Faker.fake::<LocalizedProductView>();
        input.images = Default::default();

        let actual = service
            .personalize_prohibited_content(&Faker.fake(), input)
            .await
            .unwrap();

        assert!(actual.user_state.unwrap().consent);
    }

    #[tokio::test]
    async fn should_personalize_prohibited_content_looks_up_user_when_unsafe_image_and_user_has_consent()
     {
        let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .times(1)
            .return_once(move |_| Box::pin(async move { Ok(make_test_user(true)) }));

        let notification_service = MockNotificationService::default();
        let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let mut input = Faker.fake::<LocalizedProductView>();
        input.images = vec![make_safe_image(), make_unsafe_image()]
            .into_iter()
            .collect();

        let actual = service
            .personalize_prohibited_content(&Faker.fake(), input)
            .await
            .unwrap();

        assert!(actual.user_state.unwrap().consent);
    }

    #[tokio::test]
    async fn should_personalize_prohibited_content_looks_up_user_when_unsafe_image_and_user_has_no_consent()
     {
        let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .times(1)
            .return_once(move |_| Box::pin(async move { Ok(make_test_user(false)) }));

        let notification_service = MockNotificationService::default();
        let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let mut input = Faker.fake::<LocalizedProductView>();
        input.images = vec![make_safe_image(), make_unsafe_image()]
            .into_iter()
            .collect();

        let actual = service
            .personalize_prohibited_content(&Faker.fake(), input)
            .await
            .unwrap();

        assert!(!actual.user_state.unwrap().consent);
    }

    #[tokio::test]
    async fn should_personalize_prohibited_content_looks_up_user_when_unknown_image() {
        let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .times(1)
            .return_once(move |_| Box::pin(async move { Ok(make_test_user(true)) }));

        let notification_service = MockNotificationService::default();
        let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let mut input = Faker.fake::<LocalizedProductView>();
        input.images = vec![make_unknown_image()].into_iter().collect();

        let actual = service
            .personalize_prohibited_content(&Faker.fake(), input)
            .await
            .unwrap();

        assert!(actual.user_state.unwrap().consent);
    }

    #[tokio::test]
    async fn should_personalize_all_prohibited_content_no_lookup_when_all_safe() {
        let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        let mut user_service = MockUserService::default();
        user_service.expect_find_user().never();

        let notification_service = MockNotificationService::default();
        let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let mut input1 = Faker.fake::<LocalizedProductView>();
        input1.images = vec![make_safe_image()].into_iter().collect();
        let mut input2 = Faker.fake::<LocalizedProductView>();
        input2.images = vec![make_safe_image(), make_safe_image()]
            .into_iter()
            .collect();

        let actual = service
            .personalize_all_prohibited_content(&Faker.fake(), vec![input1, input2])
            .await
            .unwrap();

        assert_eq!(actual.len(), 2);
        assert!(actual[0].user_state.unwrap().consent);
        assert!(actual[1].user_state.unwrap().consent);
    }

    #[tokio::test]
    async fn should_personalize_all_prohibited_content_lookup_once_when_any_unsafe() {
        let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .times(1)
            .return_once(move |_| Box::pin(async move { Ok(make_test_user(false)) }));

        let notification_service = MockNotificationService::default();
        let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let mut input1 = Faker.fake::<LocalizedProductView>();
        input1.images = vec![make_safe_image()].into_iter().collect();
        let mut input2 = Faker.fake::<LocalizedProductView>();
        input2.images = vec![make_unsafe_image()].into_iter().collect();
        let mut input3 = Faker.fake::<LocalizedProductView>();
        input3.images = vec![make_safe_image()].into_iter().collect();

        let actual = service
            .personalize_all_prohibited_content(&Faker.fake(), vec![input1, input2, input3])
            .await
            .unwrap();

        assert_eq!(actual.len(), 3);
        assert!(!actual[0].user_state.unwrap().consent);
        assert!(!actual[1].user_state.unwrap().consent);
        assert!(!actual[2].user_state.unwrap().consent);
    }

    #[tokio::test]
    async fn should_personalize_combines_watchlist_and_prohibited_content() {
        let product_id = ProductId::new();

        let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        watchlist_repository
            .expect_get_watchlist_record()
            .return_once(move |user_id, _, _| {
                let user_id = *user_id;
                Box::pin(async move {
                    let watched = WatchlistProductRecord {
                        shop_id: Faker.fake(),
                        shops_product_id: Faker.fake(),
                        product_id,
                        notifications: true,
                        state: common::resource_state::record::ResourceStateRecord::Active,
                        created_by: ActorRecord::System,
                        updated_by: ActorRecord::System,
                        created: OffsetDateTime::now_utc(),
                        updated: OffsetDateTime::now_utc(),
                        pk: "dummy".to_owned(),
                        sk: "dummy".to_owned(),
                        lsi1_sk: "dummy".to_owned(),
                        gsi1_pk: "dummy".to_owned(),
                        gsi1_sk: "dummy".to_owned(),
                        user_id,
                    };
                    Ok(Some(watched))
                })
            });

        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .times(1)
            .return_once(move |_| Box::pin(async move { Ok(make_test_user(true)) }));

        let mut notification_service = MockNotificationService::default();
        notification_service
            .expect_find_notifications_by_product()
            .returning(|_, _, _, _| Box::pin(async { Ok(vec![]) }));
        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        search_filter_repository
            .expect_query_user_search_filter_match_records_for_product()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let mut input = Faker.fake::<LocalizedProductView>();
        input.product_id = product_id;
        input.images = vec![make_unsafe_image()].into_iter().collect();

        let actual = service.personalize(&Faker.fake(), input).await.unwrap();

        let state = actual.user_state.unwrap();
        assert!(state.watchlist.watching);
        assert!(state.watchlist.notifications);
        assert!(state.prohibited_content.consent);
        assert!(state.notification.seen);
        assert!(!state.search_filter.matched);
    }

    #[tokio::test]
    async fn should_personalize_all_combines_watchlist_and_prohibited_content() {
        let product1_id = ProductId::new();
        let product2_id = ProductId::new();

        let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        watchlist_repository
            .expect_query_watchlist_records_all()
            .return_once(move |user_id, _| {
                let user_id = *user_id;
                Box::pin(async move {
                    let watched = vec![WatchlistProductRecord {
                        shop_id: Faker.fake(),
                        shops_product_id: Faker.fake(),
                        product_id: product1_id,
                        notifications: true,
                        state: common::resource_state::record::ResourceStateRecord::Active,
                        created_by: ActorRecord::System,
                        updated_by: ActorRecord::System,
                        created: OffsetDateTime::now_utc(),
                        updated: OffsetDateTime::now_utc(),
                        pk: "dummy".to_owned(),
                        sk: "dummy".to_owned(),
                        lsi1_sk: "dummy".to_owned(),
                        gsi1_pk: "dummy".to_owned(),
                        gsi1_sk: "dummy".to_owned(),
                        user_id,
                    }];
                    Ok(watched)
                })
            });

        let mut user_service = MockUserService::default();
        user_service
            .expect_find_user()
            .times(1)
            .return_once(move |_| Box::pin(async move { Ok(make_test_user(false)) }));

        let mut notification_service = MockNotificationService::default();
        notification_service
            .expect_find_notifications_by_product()
            .returning(|_, _, _, _| Box::pin(async { Ok(vec![]) }));
        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        search_filter_repository
            .expect_query_user_search_filter_match_records_all()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let mut input1 = Faker.fake::<LocalizedProductView>();
        input1.product_id = product1_id;
        input1.images = vec![make_safe_image()].into_iter().collect();

        let mut input2 = Faker.fake::<LocalizedProductView>();
        input2.product_id = product2_id;
        input2.images = vec![make_unsafe_image()].into_iter().collect();

        let actual = service
            .personalize_all(&Faker.fake(), vec![input1, input2])
            .await
            .unwrap();

        assert_eq!(actual.len(), 2);

        let state0 = actual[0].user_state.clone().unwrap();
        assert!(state0.watchlist.watching);
        assert!(state0.watchlist.notifications);
        assert!(!state0.prohibited_content.consent);
        assert!(state0.notification.seen);
        assert!(!state0.search_filter.matched);

        let state1 = actual[1].user_state.clone().unwrap();
        assert!(!state1.watchlist.watching);
        assert!(!state1.watchlist.notifications);
        assert!(!state1.prohibited_content.consent);
        assert!(state1.notification.seen);
        assert!(!state1.search_filter.matched);
    }

    #[tokio::test]
    async fn should_personalize_product_notification_seen_true_when_no_notifications() {
        let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        let user_service = MockUserService::default();
        let mut notification_service = MockNotificationService::default();
        notification_service
            .expect_find_notifications_by_product()
            .returning(|_, _, _, _| Box::pin(async { Ok(vec![]) }));

        let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let input = Faker.fake::<LocalizedProductView>();
        let actual = service
            .personalize_product_notification(&Faker.fake(), input)
            .await
            .unwrap();

        assert!(actual.user_state.unwrap().seen);
        assert!(actual.user_state.unwrap().origin_event_id.is_none());
    }

    #[tokio::test]
    async fn should_personalize_product_notification_seen_false_when_latest_unseen() {
        let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        let user_service = MockUserService::default();
        let mut notification_service = MockNotificationService::default();
        notification_service
            .expect_find_notifications_by_product()
            .returning(|_, _, _, _| Box::pin(async { Ok(vec![make_test_notification(false)]) }));

        let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let input = Faker.fake::<LocalizedProductView>();
        let actual = service
            .personalize_product_notification(&Faker.fake(), input)
            .await
            .unwrap();

        assert!(!actual.user_state.unwrap().seen);
        assert!(actual.user_state.unwrap().origin_event_id.is_some());
    }

    #[tokio::test]
    async fn should_personalize_product_notification_seen_true_when_latest_seen() {
        let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        let user_service = MockUserService::default();
        let mut notification_service = MockNotificationService::default();
        notification_service
            .expect_find_notifications_by_product()
            .returning(|_, _, _, _| Box::pin(async { Ok(vec![make_test_notification(true)]) }));

        let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let input = Faker.fake::<LocalizedProductView>();
        let actual = service
            .personalize_product_notification(&Faker.fake(), input)
            .await
            .unwrap();

        assert!(actual.user_state.unwrap().seen);
        assert!(actual.user_state.unwrap().origin_event_id.is_some());
    }

    #[tokio::test]
    async fn should_personalize_product_notification_all_mixed_states() {
        let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        let user_service = MockUserService::default();
        let product1_id = ProductId::new();
        let product2_id = ProductId::new();
        let product3_id = ProductId::new();

        let mut notification_service = MockNotificationService::default();
        notification_service
            .expect_find_notifications_by_product()
            .returning(move |_, product_id, _, _| {
                let pid = *product_id;
                let p1 = product1_id;
                let p2 = product2_id;
                Box::pin(async move {
                    if pid == p1 {
                        Ok(vec![make_test_notification(false)])
                    } else if pid == p2 {
                        Ok(vec![make_test_notification(true)])
                    } else {
                        Ok(vec![])
                    }
                })
            });

        let search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let mut input1 = Faker.fake::<LocalizedProductView>();
        input1.product_id = product1_id;
        let mut input2 = Faker.fake::<LocalizedProductView>();
        input2.product_id = product2_id;
        let mut input3 = Faker.fake::<LocalizedProductView>();
        input3.product_id = product3_id;

        let actual = service
            .personalize_product_notification_all(&Faker.fake(), vec![input1, input2, input3])
            .await
            .unwrap();

        assert_eq!(actual.len(), 3);
        assert!(!actual[0].user_state.unwrap().seen);
        assert!(actual[0].user_state.unwrap().origin_event_id.is_some());
        assert!(actual[1].user_state.unwrap().seen);
        assert!(actual[1].user_state.unwrap().origin_event_id.is_some());
        assert!(actual[2].user_state.unwrap().seen);
        assert!(actual[2].user_state.unwrap().origin_event_id.is_none());
    }

    // ---- Search filter personalization tests ----

    #[tokio::test]
    async fn should_personalize_search_filter_matched_when_record_exists() {
        let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        let mut user_service = MockUserService::default();
        user_service.expect_find_user().returning(|_| {
            Box::pin(async {
                let mut user: User = Faker.fake();
                user.tier = user::core::tier::UserTier::Ultimate;
                Ok(user)
            })
        });
        let notification_service = MockNotificationService::default();

        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        let match_record: search_filter::dynamodb::user_search_filter_match_record::UserSearchFilterMatchRecord = Faker.fake();
        let expected_filter_id = match_record.user_search_filter_id;
        let expected_name = match_record.user_search_filter_name.clone();
        let expected_reason = match_record.enhanced_match_reason.clone();
        let expected_product_id = match_record.product_id;

        search_filter_repository
            .expect_query_user_search_filter_match_records_for_product()
            .returning(move |_, _, _| {
                let record = match_record.clone();
                Box::pin(async move { Ok(vec![record]) })
            });

        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let mut input = Faker.fake::<LocalizedProductView>();
        input.product_id = expected_product_id;

        let actual = service
            .personalize_search_filter(&Faker.fake(), input)
            .await
            .unwrap();

        let state = actual.user_state.unwrap();
        assert!(state.matched);
        assert_eq!(state.user_search_filter_id, Some(expected_filter_id));
        assert_eq!(
            state.user_search_filter_name.map(String::from),
            expected_name
        );
        assert_eq!(state.match_reason.map(String::from), expected_reason);
    }

    #[tokio::test]
    async fn should_personalize_search_filter_not_matched_when_no_records() {
        let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        let user_service = MockUserService::default();
        let notification_service = MockNotificationService::default();

        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        search_filter_repository
            .expect_query_user_search_filter_match_records_for_product()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let input = Faker.fake::<LocalizedProductView>();
        let actual = service
            .personalize_search_filter(&Faker.fake(), input)
            .await
            .unwrap();

        let state = actual.user_state.unwrap();
        assert!(!state.matched);
        assert!(state.user_search_filter_id.is_none());
        assert!(state.user_search_filter_name.is_none());
        assert!(state.match_reason.is_none());
    }

    #[tokio::test]
    async fn should_personalize_search_filter_all_mixed_states() {
        let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        let mut user_service = MockUserService::default();
        user_service.expect_find_user().returning(|_| {
            Box::pin(async {
                let mut user: User = Faker.fake();
                user.tier = user::core::tier::UserTier::Ultimate;
                Ok(user)
            })
        });
        let notification_service = MockNotificationService::default();

        let product1_id = ProductId::new();
        let product2_id = ProductId::new();

        let mut match_record: search_filter::dynamodb::user_search_filter_match_record::UserSearchFilterMatchRecord = Faker.fake();
        match_record.product_id = product1_id;
        let expected_filter_id = match_record.user_search_filter_id;
        let expected_name = match_record.user_search_filter_name.clone();

        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        search_filter_repository
            .expect_query_user_search_filter_match_records_all()
            .return_once(move |_| {
                let record = match_record;
                Box::pin(async move { Ok(vec![record]) })
            });

        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let mut input1 = Faker.fake::<LocalizedProductView>();
        input1.product_id = product1_id;
        let mut input2 = Faker.fake::<LocalizedProductView>();
        input2.product_id = product2_id;

        let actual = service
            .personalize_search_filter_all(&Faker.fake(), vec![input1, input2])
            .await
            .unwrap();

        assert_eq!(actual.len(), 2);
        let state0 = actual[0].user_state.clone().unwrap();
        assert!(state0.matched);
        assert_eq!(state0.user_search_filter_id, Some(expected_filter_id));
        assert_eq!(
            state0.user_search_filter_name.map(String::from),
            expected_name
        );

        let state1 = actual[1].user_state.clone().unwrap();
        assert!(!state1.matched);
        assert!(state1.user_search_filter_id.is_none());
        assert!(state1.user_search_filter_name.is_none());
    }

    // ---- Quota-aware search filter personalization tests ----

    #[tokio::test]
    async fn should_personalize_search_filter_hidden_when_quota_exceeded_for_match() {
        let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        let notification_service = MockNotificationService::default();

        let mut user_service = MockUserService::default();
        user_service.expect_find_user().returning(|_| {
            Box::pin(async {
                let mut user: User = Faker.fake();
                user.tier = user::core::tier::UserTier::Free;
                Ok(user)
            })
        });

        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        let mut match_record: search_filter::dynamodb::user_search_filter_match_record::UserSearchFilterMatchRecord = Faker.fake();
        match_record.created = OffsetDateTime::now_utc();
        let expected_product_id = match_record.product_id;

        search_filter_repository
            .expect_query_user_search_filter_match_records_for_product()
            .returning(move |_, _, _| {
                let record = match_record.clone();
                Box::pin(async move { Ok(vec![record]) })
            });

        // Position 11 means this match is the 11th in its month, exceeding Free quota of 10
        search_filter_repository
            .expect_count_user_search_filter_match_records_for_between()
            .returning(|_, _, _| Box::pin(async { Ok(11) }));

        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let mut input = Faker.fake::<LocalizedProductView>();
        input.product_id = expected_product_id;
        input.title.localization = common::language::domain::Language::En;

        let actual = service
            .personalize_search_filter(&Faker.fake(), input)
            .await
            .unwrap();

        let state = actual.user_state.unwrap();
        assert!(state.matched);
        assert!(state.hidden);
        assert_eq!(
            actual.item.product_id,
            common::product_id::ProductId::from(uuid::Uuid::nil())
        );
        assert_eq!(
            actual.item.title.payload.to_string(),
            "Hidden Product Title"
        );
        assert_eq!(
            actual.item.state,
            common::product_state::domain::ProductState::Unknown
        );
        assert!(actual.item.images.is_empty());
        assert!(actual.item.description.is_none());
        assert!(actual.item.price.is_none());
        assert_eq!(actual.item.created, OffsetDateTime::UNIX_EPOCH);
        assert_eq!(actual.item.updated, OffsetDateTime::UNIX_EPOCH);
    }

    #[tokio::test]
    async fn should_personalize_search_filter_not_hidden_when_within_quota() {
        let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        let notification_service = MockNotificationService::default();

        let mut user_service = MockUserService::default();
        user_service.expect_find_user().returning(|_| {
            Box::pin(async {
                let mut user: User = Faker.fake();
                user.tier = user::core::tier::UserTier::Free;
                Ok(user)
            })
        });

        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        let mut match_record: search_filter::dynamodb::user_search_filter_match_record::UserSearchFilterMatchRecord = Faker.fake();
        match_record.created = OffsetDateTime::now_utc();
        let expected_product_id = match_record.product_id;
        let expected_filter_id = match_record.user_search_filter_id;

        search_filter_repository
            .expect_query_user_search_filter_match_records_for_product()
            .returning(move |_, _, _| {
                let record = match_record.clone();
                Box::pin(async move { Ok(vec![record]) })
            });

        // Position 5 means this match is the 5th in its month, within Free quota of 10
        search_filter_repository
            .expect_count_user_search_filter_match_records_for_between()
            .returning(|_, _, _| Box::pin(async { Ok(5) }));

        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let mut input = Faker.fake::<LocalizedProductView>();
        input.product_id = expected_product_id;
        let original_title = input.title.payload.to_string();

        let actual = service
            .personalize_search_filter(&Faker.fake(), input)
            .await
            .unwrap();

        let state = actual.user_state.unwrap();
        assert!(state.matched);
        assert!(!state.hidden);
        assert_eq!(state.user_search_filter_id, Some(expected_filter_id));
        assert_eq!(actual.item.product_id, expected_product_id);
        assert_eq!(actual.item.title.payload.to_string(), original_title);
    }

    #[tokio::test]
    async fn should_personalize_search_filter_hidden_when_quota_exceeded_for_previous_month_match()
    {
        let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        let notification_service = MockNotificationService::default();

        let mut user_service = MockUserService::default();
        user_service.expect_find_user().returning(|_| {
            Box::pin(async {
                let mut user: User = Faker.fake();
                user.tier = user::core::tier::UserTier::Free;
                Ok(user)
            })
        });

        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        let mut match_record: search_filter::dynamodb::user_search_filter_match_record::UserSearchFilterMatchRecord = Faker.fake();
        match_record.created = time::macros::datetime!(2020-01-15 12:00:00 UTC);
        let expected_product_id = match_record.product_id;

        search_filter_repository
            .expect_query_user_search_filter_match_records_for_product()
            .returning(move |_, _, _| {
                let record = match_record.clone();
                Box::pin(async move { Ok(vec![record]) })
            });

        // Position 11 means this previous-month match exceeds that month's Free quota of 10.
        search_filter_repository
            .expect_count_user_search_filter_match_records_for_between()
            .returning(|_, _, _| Box::pin(async { Ok(11) }));

        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let mut input = Faker.fake::<LocalizedProductView>();
        input.product_id = expected_product_id;
        input.title.localization = common::language::domain::Language::En;

        let actual = service
            .personalize_search_filter(&Faker.fake(), input)
            .await
            .unwrap();

        let state = actual.user_state.unwrap();
        assert!(state.matched);
        assert!(state.hidden);
        assert_eq!(
            actual.item.product_id,
            common::product_id::ProductId::from(uuid::Uuid::nil())
        );
        assert_eq!(
            actual.item.title.payload.to_string(),
            "Hidden Product Title"
        );
    }

    #[tokio::test]
    async fn should_personalize_search_filter_all_hide_only_matches_beyond_quota() {
        let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        let notification_service = MockNotificationService::default();

        let mut user_service = MockUserService::default();
        user_service.expect_find_user().returning(|_| {
            Box::pin(async {
                let mut user: User = Faker.fake();
                user.tier = user::core::tier::UserTier::Free;
                Ok(user)
            })
        });

        let now = OffsetDateTime::now_utc();
        let within_quota_product_id = ProductId::new();
        let beyond_quota_product_id = ProductId::new();
        let old_product_id = ProductId::new();
        let unmatched_product_id = ProductId::new();

        // Anchor all current-month timestamps to the first of the current month.
        // Using fixed offsets from month_start ensures correct ordering regardless
        // of when the test runs (avoids crossing month boundaries with large offsets from `now`).
        let current_month_base = OffsetDateTime::new_utc(
            time::Date::from_calendar_date(now.year(), now.month(), 1).unwrap(),
            time::Time::MIDNIGHT,
        );

        // Current-month match that remains within that month's Free quota of 10.
        let mut within_match: search_filter::dynamodb::user_search_filter_match_record::UserSearchFilterMatchRecord = Faker.fake();
        within_match.product_id = within_quota_product_id;
        within_match.created = current_month_base;

        // Current-month match that lands beyond that month's Free quota of 10.
        let mut beyond_match: search_filter::dynamodb::user_search_filter_match_record::UserSearchFilterMatchRecord = Faker.fake();
        beyond_match.product_id = beyond_quota_product_id;
        beyond_match.created = current_month_base + time::Duration::hours(11);

        // Old match from a previous month — counted against its own month.
        let mut old_match: search_filter::dynamodb::user_search_filter_match_record::UserSearchFilterMatchRecord = Faker.fake();
        old_match.product_id = old_product_id;
        old_match.created = time::macros::datetime!(2020-01-15 12:00:00 UTC);

        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        search_filter_repository
            .expect_query_user_search_filter_match_records_all()
            .return_once(move |_| {
                let mut records = Vec::new();
                // 10 filler matches filling up the quota (created between within_match and beyond_match)
                for i in 0..10i64 {
                    let mut filler: search_filter::dynamodb::user_search_filter_match_record::UserSearchFilterMatchRecord = Faker.fake();
                    filler.created = current_month_base + time::Duration::hours(i + 1);
                    records.push(filler);
                }
                records.push(within_match);
                records.push(beyond_match);
                records.push(old_match);
                Box::pin(async move { Ok(records) })
            });

        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let mut input1 = Faker.fake::<LocalizedProductView>();
        input1.product_id = within_quota_product_id;
        let mut input2 = Faker.fake::<LocalizedProductView>();
        input2.product_id = beyond_quota_product_id;
        let mut input3 = Faker.fake::<LocalizedProductView>();
        input3.product_id = old_product_id;
        let mut input4 = Faker.fake::<LocalizedProductView>();
        input4.product_id = unmatched_product_id;

        let actual = service
            .personalize_search_filter_all(&Faker.fake(), vec![input1, input2, input3, input4])
            .await
            .unwrap();

        assert_eq!(actual.len(), 4);

        // Quota is enforced per month:
        // old_match is position 1 in its month → visible
        // within_match is position 1 in the current month → visible
        // beyond_match is position 12 in the current month → hidden

        let state0 = actual[0].user_state.clone().unwrap();
        assert!(state0.matched);
        assert!(!state0.hidden);
        assert_eq!(actual[0].item.product_id, within_quota_product_id);

        let state1 = actual[1].user_state.clone().unwrap();
        assert!(state1.matched);
        assert!(state1.hidden);
        assert_eq!(
            actual[1].item.product_id,
            common::product_id::ProductId::from(uuid::Uuid::nil())
        );

        let state2 = actual[2].user_state.clone().unwrap();
        assert!(state2.matched);
        assert!(!state2.hidden);
        assert_eq!(actual[2].item.product_id, old_product_id);

        let state3 = actual[3].user_state.clone().unwrap();
        assert!(!state3.matched);
        assert!(!state3.hidden);
        assert_eq!(actual[3].item.product_id, unmatched_product_id);
    }

    #[tokio::test]
    async fn should_personalize_search_filter_all_hide_previous_month_matches_beyond_quota() {
        let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        let notification_service = MockNotificationService::default();

        let mut user_service = MockUserService::default();
        user_service.expect_find_user().returning(|_| {
            Box::pin(async {
                let mut user: User = Faker.fake();
                user.tier = user::core::tier::UserTier::Free;
                Ok(user)
            })
        });

        let old_product_id = ProductId::new();
        let mut old_match: search_filter::dynamodb::user_search_filter_match_record::UserSearchFilterMatchRecord = Faker.fake();
        old_match.product_id = old_product_id;
        old_match.created =
            time::macros::datetime!(2020-01-15 12:00:00 UTC) + time::Duration::hours(10);

        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::default();
        search_filter_repository
            .expect_query_user_search_filter_match_records_all()
            .return_once(move |_| {
                let mut records = Vec::new();
                for i in 0..10i64 {
                    let mut filler: search_filter::dynamodb::user_search_filter_match_record::UserSearchFilterMatchRecord = Faker.fake();
                    filler.created =
                        time::macros::datetime!(2020-01-15 12:00:00 UTC) + time::Duration::hours(i);
                    records.push(filler);
                }
                records.push(old_match);
                Box::pin(async move { Ok(records) })
            });

        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
        );

        let mut input = Faker.fake::<LocalizedProductView>();
        input.product_id = old_product_id;
        input.title.localization = common::language::domain::Language::En;

        let actual = service
            .personalize_search_filter_all(&Faker.fake(), vec![input])
            .await
            .unwrap();

        assert_eq!(actual.len(), 1);
        let state = actual[0].user_state.clone().unwrap();
        assert!(state.matched);
        assert!(state.hidden);
        assert_eq!(
            actual[0].item.product_id,
            common::product_id::ProductId::from(uuid::Uuid::nil())
        );
        assert_eq!(
            actual[0].item.title.payload.to_string(),
            "Hidden Product Title"
        );
    }

    #[test]
    fn should_return_month_start_for_match_created_in_past_month() {
        let actual = month_start(&time::macros::datetime!(2020-01-15 12:34:56.789 UTC));

        assert_eq!(actual, time::macros::datetime!(2020-01-01 00:00:00 UTC));
    }

    #[test]
    fn should_anonymize_product_with_correct_hidden_values() {
        use crate::service::anonymize_product;
        let mut product = Faker.fake::<LocalizedProductView>();
        product.title.localization = common::language::domain::Language::En;

        anonymize_product(&mut product);

        assert_eq!(
            product.product_id,
            common::product_id::ProductId::from(uuid::Uuid::nil())
        );
        assert_eq!(
            product.shop_id,
            common::shop_id::ShopId::from(uuid::Uuid::nil())
        );
        assert_eq!(
            product.seller_id,
            common::shop_id::ShopId::from(uuid::Uuid::nil())
        );
        assert_eq!(product.title.payload.to_string(), "Hidden Product Title");
        assert!(product.description.is_none());
        assert!(product.price.is_none());
        assert!(product.price_estimate_min.is_none());
        assert!(product.price_estimate_max.is_none());
        assert_eq!(
            product.state,
            common::product_state::domain::ProductState::Unknown
        );
        assert!(product.images.is_empty());
        assert!(product.auction_start.is_none());
        assert!(product.auction_end.is_none());
        assert_eq!(product.created, OffsetDateTime::UNIX_EPOCH);
        assert_eq!(product.updated, OffsetDateTime::UNIX_EPOCH);
        assert_eq!(
            product.shop_name,
            common::shop_name::ShopName::from("Hidden")
        );
        assert_eq!(
            product.seller_name,
            common::shop_name::ShopName::from("Hidden")
        );
    }

    #[test]
    fn should_provide_language_specific_hidden_titles() {
        use crate::service::hidden_title;
        use common::language::domain::Language;

        assert_eq!(
            hidden_title(Language::De).to_string(),
            "Versteckter Produkttitel"
        );
        assert_eq!(
            hidden_title(Language::En).to_string(),
            "Hidden Product Title"
        );
        assert_eq!(
            hidden_title(Language::Fr).to_string(),
            "Titre du produit masqué"
        );
        assert_eq!(
            hidden_title(Language::Es).to_string(),
            "Título de producto oculto"
        );
        assert_eq!(
            hidden_title(Language::It).to_string(),
            "Titolo del prodotto nascosto"
        );
    }
}
