use common::{api::error::ApiError, personalized::Personalized, user_id::UserId};
use product::core::{
    product::LocalizedProductView,
    user_state::{ProductUserState, ProhibitedContentUserState, WatchlistUserState},
};
use product_watchlist::{
    dynamodb::repository::WatchlistProductDynamoDbRepository,
    service::product_watchlist_service::WatchProductError,
};
use std::collections::HashMap;
use user::service::user_service::{UserService, UserServiceError};

#[derive(Debug, thiserror::Error)]
pub enum ProductPersonalizationError {
    #[error("WatchProductError: {0}")]
    WatchProductError(#[from] WatchProductError),
    #[error("UserServiceError: {0}")]
    UserServiceError(#[from] UserServiceError),
}

impl From<ProductPersonalizationError> for ApiError {
    fn from(value: ProductPersonalizationError) -> Self {
        match value {
            ProductPersonalizationError::WatchProductError(e) => e.into(),
            ProductPersonalizationError::UserServiceError(_) => ApiError::internal_server_error(
                common::api::error_code::INTERNAL_SERVER_ERROR,
                Box::new(value),
            ),
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
    user_service: &'a (dyn UserService + Sync),
}

impl<'a> ProductPersonalizationServiceImpl<'a> {
    pub fn new(
        watchlist_repository: &'a (dyn WatchlistProductDynamoDbRepository + Sync),
        user_service: &'a (dyn UserService + Sync),
    ) -> Self {
        Self {
            watchlist_repository,
            user_service,
        }
    }
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
        Ok(Personalized {
            item: prohibited_content.item,
            user_state: Some(ProductUserState {
                watchlist: watchlist.user_state.unwrap_or_default(),
                prohibited_content: prohibited_content.user_state.unwrap_or_default(),
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

        let result = watchlist_results
            .into_iter()
            .map(|p| Personalized {
                item: p.item,
                user_state: Some(ProductUserState {
                    watchlist: p.user_state.unwrap_or_default(),
                    prohibited_content: ProhibitedContentUserState { consent },
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
    use time::OffsetDateTime;
    use user::core::user::User;
    use user::service::user_service::MockUserService;

    use crate::service::{ProductPersonalizationService, ProductPersonalizationServiceImpl};

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
        let service = ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);

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
        let service = ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);

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
        let service = ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);

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
        let service = ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);

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
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        }
    }

    #[tokio::test]
    async fn should_personalize_prohibited_content_consent_true_when_all_images_safe() {
        let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
        let mut user_service = MockUserService::default();
        user_service.expect_find_user().never();

        let service = ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);

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

        let service = ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);

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

        let service = ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);

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

        let service = ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);

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

        let service = ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);

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

        let service = ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);

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

        let service = ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);

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

        let service = ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);

        let mut input = Faker.fake::<LocalizedProductView>();
        input.product_id = product_id;
        input.images = vec![make_unsafe_image()];

        let actual = service.personalize(&Faker.fake(), input).await.unwrap();

        let state = actual.user_state.unwrap();
        assert!(state.watchlist.watching);
        assert!(state.watchlist.notifications);
        assert!(state.prohibited_content.consent);
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

        let service = ProductPersonalizationServiceImpl::new(&watchlist_repository, &user_service);

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

        let state0 = actual[0].user_state.unwrap();
        assert!(state0.watchlist.watching);
        assert!(state0.watchlist.notifications);
        assert!(!state0.prohibited_content.consent);

        let state1 = actual[1].user_state.unwrap();
        assert!(!state1.watchlist.watching);
        assert!(!state1.watchlist.notifications);
        assert!(!state1.prohibited_content.consent);
    }
}
