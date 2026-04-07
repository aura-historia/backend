use common::{
    api::error::ApiError, enhanced_match_reason::EnhancedMatchReason, language::domain::Language,
    localized::Localized, personalized::Personalized, product_id::ProductId,
    product_state::domain::ProductState, user_id::UserId,
    user_search_filter_name::UserSearchFilterName,
};
use notification::service::notification_service::{NotificationError, NotificationService};
use product::core::{
    authenticity::Authenticity,
    condition::Condition,
    product::LocalizedProductView,
    provenance::Provenance,
    restoration::Restoration,
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
use search_filter::service::user_search_filter_service::UserSearchFilterService;
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
    user_search_filter_service: &'a (dyn UserSearchFilterService + Sync),
}

impl<'a> ProductPersonalizationServiceImpl<'a> {
    pub fn new(
        watchlist_repository: &'a (dyn WatchlistProductDynamoDbRepository + Sync),
        notification_service: &'a (dyn NotificationService + Sync),
        user_service: &'a (dyn UserService + Sync),
        search_filter_repository: &'a (dyn UserSearchFilterDynamoDbRepository + Sync),
        user_search_filter_service: &'a (dyn UserSearchFilterService + Sync),
    ) -> Self {
        Self {
            watchlist_repository,
            notification_service,
            user_service,
            search_filter_repository,
            user_search_filter_service,
        }
    }

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

    async fn is_search_filter_match_quota_exceeded(
        &self,
        user_id: &UserId,
    ) -> Result<bool, ProductPersonalizationError> {
        let user = self.user_service.find_user(user_id).await?;
        let quota = user.tier.search_filter_match_quota();
        if quota == u32::MAX {
            return Ok(false);
        }
        let count = self
            .user_search_filter_service
            .count_user_search_filter_matches_for_this_month(user_id)
            .await
            .map_err(|e| ProductPersonalizationError::UserSearchFilterError(e.to_string()))?;
        Ok(count as u32 >= quota)
    }
}

fn hidden_title(language: Language) -> Title {
    match language {
        Language::De => Title::from("Versteckter Produkttitel"),
        Language::En => Title::from("Hidden Product Title"),
        Language::Fr => Title::from("Titre du produit masqué"),
        Language::Es => Title::from("Título de producto oculto"),
        Language::It => Title::from("Titolo del prodotto nascosto"),
    }
}

fn anonymize_product(product: &mut LocalizedProductView) {
    let nil = uuid::Uuid::nil();
    product.product_id = ProductId::from(nil);
    product.product_slug_id = common::slug_id::SlugId::raw("");
    product.shop_slug_id = common::slug_id::SlugId::raw("");
    product.seller_slug_id = common::slug_id::SlugId::raw("");
    product.event_id = common::event_id::EventId::from(nil);
    product.shop_id = common::shop_id::ShopId::from(nil);
    product.seller_id = common::shop_id::ShopId::from(nil);
    product.shops_product_id = common::shops_product_id::ShopsProductId::from(
        "00000000-0000-0000-0000-000000000000".to_string(),
    );
    product.shop_name = common::shop_name::ShopName::from("Hidden");
    product.seller_name = common::shop_name::ShopName::from("Hidden");
    product.category_id = None;
    product.category_name = None;
    product.period_id = None;
    product.period_name = None;
    let lang = product.title.localization;
    product.title = Localized::new(lang, hidden_title(lang));
    product.description = None;
    product.price = None;
    product.price_estimate_min = None;
    product.price_estimate_max = None;
    product.state = ProductState::Unknown;
    product.url = url::Url::parse("https://hidden.example.com").expect("valid url");
    product.images = vec![];
    product.origin_year = None;
    product.authenticity = Authenticity::Unknown;
    product.condition = Condition::Unknown;
    product.provenance = Provenance::Unknown;
    product.restoration = Restoration::Unknown;
    product.auction_start = None;
    product.auction_end = None;
    product.created = OffsetDateTime::UNIX_EPOCH;
    product.updated = OffsetDateTime::UNIX_EPOCH;
}

fn is_current_month(dt: &OffsetDateTime) -> bool {
    let now = OffsetDateTime::now_utc();
    dt.year() == now.year() && dt.month() == now.month()
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
                let has_current_month_match = is_current_month(&record.created);
                let hidden = if has_current_month_match {
                    self.is_search_filter_match_quota_exceeded(user_id).await?
                } else {
                    false
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

        let any_current_month = match_by_product
            .values()
            .any(|record| is_current_month(&record.created));

        let quota_exceeded = if any_current_month {
            self.is_search_filter_match_quota_exceeded(user_id).await?
        } else {
            false
        };

        let result = products
            .into_iter()
            .map(|mut product| {
                let search_filter_state = match match_by_product.get(&product.product_id) {
                    Some(record) => {
                        let hidden = quota_exceeded && is_current_month(&record.created);
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
    use fake::{Fake, Faker};
    use product::core::product::LocalizedProductView;
    use product::core::product_image::ProductImage;
    use product::core::prohibited_content::ProhibitedContent;
    use product_watchlist::dynamodb::{
        record::WatchlistProductRecord, repository::MockWatchlistProductDynamoDbRepository,
    };
    use search_filter::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
    use search_filter::service::user_search_filter_service::MockUserSearchFilterService;
    use time::OffsetDateTime;
    use user::core::user::User;
    use user::service::user_service::MockUserService;

    use crate::service::{ProductPersonalizationService, ProductPersonalizationServiceImpl};
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
        let user_search_filter_service = MockUserSearchFilterService::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
            &user_search_filter_service,
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
        let user_search_filter_service = MockUserSearchFilterService::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
            &user_search_filter_service,
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
        let user_search_filter_service = MockUserSearchFilterService::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
            &user_search_filter_service,
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
        let user_search_filter_service = MockUserSearchFilterService::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
            &user_search_filter_service,
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
            prohibited_content_consent,
            tier: user::core::tier::UserTier::Free,
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
                watchlist_payload: NotificationWatchlistPayload::StateChange {
                    old_state: common::product_state::domain::ProductState::Listed,
                    new_state: common::product_state::domain::ProductState::Available,
                },
            },
            seen,
            external: false,
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
        let user_search_filter_service = MockUserSearchFilterService::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
            &user_search_filter_service,
        );

        let mut input = Faker.fake::<LocalizedProductView>();
        input.images = vec![make_safe_image(), make_safe_image()];

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
        let user_search_filter_service = MockUserSearchFilterService::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
            &user_search_filter_service,
        );

        let mut input = Faker.fake::<LocalizedProductView>();
        input.images = vec![];

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
        let user_search_filter_service = MockUserSearchFilterService::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
            &user_search_filter_service,
        );

        let mut input = Faker.fake::<LocalizedProductView>();
        input.images = vec![make_safe_image(), make_unsafe_image()];

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
        let user_search_filter_service = MockUserSearchFilterService::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
            &user_search_filter_service,
        );

        let mut input = Faker.fake::<LocalizedProductView>();
        input.images = vec![make_safe_image(), make_unsafe_image()];

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
        let user_search_filter_service = MockUserSearchFilterService::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
            &user_search_filter_service,
        );

        let mut input = Faker.fake::<LocalizedProductView>();
        input.images = vec![make_unknown_image()];

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
        let user_search_filter_service = MockUserSearchFilterService::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
            &user_search_filter_service,
        );

        let mut input1 = Faker.fake::<LocalizedProductView>();
        input1.images = vec![make_safe_image()];
        let mut input2 = Faker.fake::<LocalizedProductView>();
        input2.images = vec![make_safe_image(), make_safe_image()];

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
        let user_search_filter_service = MockUserSearchFilterService::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
            &user_search_filter_service,
        );

        let mut input1 = Faker.fake::<LocalizedProductView>();
        input1.images = vec![make_safe_image()];
        let mut input2 = Faker.fake::<LocalizedProductView>();
        input2.images = vec![make_unsafe_image()];
        let mut input3 = Faker.fake::<LocalizedProductView>();
        input3.images = vec![make_safe_image()];

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
        let user_search_filter_service = MockUserSearchFilterService::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
            &user_search_filter_service,
        );

        let mut input = Faker.fake::<LocalizedProductView>();
        input.product_id = product_id;
        input.images = vec![make_unsafe_image()];

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
        let user_search_filter_service = MockUserSearchFilterService::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
            &user_search_filter_service,
        );

        let mut input1 = Faker.fake::<LocalizedProductView>();
        input1.product_id = product1_id;
        input1.images = vec![make_safe_image()];

        let mut input2 = Faker.fake::<LocalizedProductView>();
        input2.product_id = product2_id;
        input2.images = vec![make_unsafe_image()];

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
        let user_search_filter_service = MockUserSearchFilterService::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
            &user_search_filter_service,
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
        let user_search_filter_service = MockUserSearchFilterService::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
            &user_search_filter_service,
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
        let user_search_filter_service = MockUserSearchFilterService::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
            &user_search_filter_service,
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
        let user_search_filter_service = MockUserSearchFilterService::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
            &user_search_filter_service,
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

        let user_search_filter_service = MockUserSearchFilterService::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
            &user_search_filter_service,
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

        let user_search_filter_service = MockUserSearchFilterService::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
            &user_search_filter_service,
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

        let user_search_filter_service = MockUserSearchFilterService::default();
        let service = ProductPersonalizationServiceImpl::new(
            &watchlist_repository,
            &notification_service,
            &user_service,
            &search_filter_repository,
            &user_search_filter_service,
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
}
