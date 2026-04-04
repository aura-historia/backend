use crate::{
    core::{quota::WatchlistQuota, watchlist_product::WatchlistProduct},
    dynamodb::{
        record::{WatchlistProductRecord, mk_gsi1_pk, mk_gsi1_sk, mk_lsi1_sk, mk_pk, mk_sk},
        record_update::WatchlistProductRecordUpdate,
        repository::WatchlistProductDynamoDbRepository,
    },
    service::{
        command::UpdateWatchlistProductCommand,
        sort_watchlist_product_field::SortWatchlistProductField,
    },
};
use aws_sdk_dynamodb::{config::http::HttpResponse, error::SdkError};
use common::slug_id::SlugId;
use common::{
    pagination::cursor::{Cursor, CursoredResult},
    price::domain::MonetaryAmountOverflowError,
    product_id::ProductId,
    shop_id::ShopId,
    shops_product_id::ShopsProductId,
    sort::{Sort, SortOrder},
    user_id::UserId,
};
use product::dynamodb::repository::ProductDynamoDbRepository;
use time::OffsetDateTime;
use user::service::user_service::{UserService, UserServiceError};

#[derive(thiserror::Error, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum WatchProductError {
    #[error("{0}")]
    MonetaryAmountOverflowError(#[from] MonetaryAmountOverflowError),

    #[error("Product with ShopId '{0}' and ShopsProductId '{1}' not found.")]
    ProductNotFound(ShopId, ShopsProductId),

    #[error("Product with ShopSlugId '{0}' and ProductSlugId '{1}' not found.")]
    ProductSlugNotFound(SlugId<0>, SlugId<6>),

    #[error("There exists no User with id '{0}'.")]
    UserNotFound(UserId),

    #[error(
        "There exists no Watchlist-Product for user '{0}' with Shop-Id '{1}' and Shops-Product-Id '{2}'."
    )]
    WatchlistProductNotFound(UserId, ShopId, ShopsProductId),

    #[error("Encountered DynamoDB SdkError for GetItem: {0}")]
    SdkGetItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for BatchGetItem: {0}")]
    SdkBatchGetItemError(
        #[from]
        SdkError<aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for QueryItem: {0}")]
    SdkQueryError(#[from] SdkError<aws_sdk_dynamodb::operation::query::QueryError, HttpResponse>),

    #[error("Encountered DynamoDB SdkError for PutItem: {0}")]
    SdkPutItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::put_item::PutItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for DeleteItem: {0}")]
    SdkDeleteItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::delete_item::DeleteItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for UpdateItem: {0}")]
    SdkUpdateItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::update_item::UpdateItemError, HttpResponse>,
    ),

    #[error("Unable to resolve unprocessed items after '{0}' retries. Failing entire operation.")]
    UnprocessedAfterMaxRetries(u32),

    #[error(
        "Exceeded the maximum amount of watchlist entries. There are already {0}/{1} watchlist entries occupied."
    )]
    WatchlistEntryCountExceeded(u32, u32),

    #[error("UserServiceError: {0}")]
    UserServiceError(UserServiceError),
}

#[cfg(feature = "data")]
pub mod api {
    use crate::service::product_watchlist_service::WatchProductError;
    use common::api::error::ApiError;
    use common::api::error_code::{
        INTERNAL_SERVER_ERROR, MONETARY_AMOUNT_OVERFLOW, PRODUCT_NOT_FOUND,
        UNPROCESSED_AFTER_MAX_RETRIES, USER_NOT_FOUND, WATCHLIST_ENTRY_NOT_FOUND,
        WATCHLIST_QUOTA_EXCEEDED,
    };

    impl From<WatchProductError> for ApiError {
        fn from(err: WatchProductError) -> Self {
            match err {
                WatchProductError::MonetaryAmountOverflowError(_) => {
                    ApiError::internal_server_error(MONETARY_AMOUNT_OVERFLOW, Box::new(err))
                }
                WatchProductError::ProductNotFound(_, _) => {
                    ApiError::not_found(PRODUCT_NOT_FOUND, Box::new(err))
                }
                WatchProductError::ProductSlugNotFound(_, _) => {
                    ApiError::not_found(PRODUCT_NOT_FOUND, Box::new(err))
                }
                WatchProductError::UserNotFound(_) => {
                    ApiError::not_found(USER_NOT_FOUND, Box::new(err))
                }
                WatchProductError::UserServiceError(_) => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err))
                }
                WatchProductError::WatchlistProductNotFound(_, _, _) => {
                    ApiError::not_found(WATCHLIST_ENTRY_NOT_FOUND, Box::new(err))
                }
                WatchProductError::SdkGetItemError(err) => err.into(),
                WatchProductError::SdkBatchGetItemError(err) => err.into(),
                WatchProductError::SdkQueryError(err) => err.into(),
                WatchProductError::SdkPutItemError(err) => err.into(),
                WatchProductError::SdkDeleteItemError(err) => err.into(),
                WatchProductError::SdkUpdateItemError(err) => err.into(),
                WatchProductError::UnprocessedAfterMaxRetries(_) => {
                    ApiError::service_unavailable(UNPROCESSED_AFTER_MAX_RETRIES, Box::new(err))
                }
                WatchProductError::WatchlistEntryCountExceeded(_, _) => {
                    ApiError::unprocessable_entity(WATCHLIST_QUOTA_EXCEEDED, Box::new(err))
                }
            }
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait ProductWatchListService {
    async fn find_watchlist_product(
        &self,
        user_id: &UserId,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<WatchlistProduct, WatchProductError>;

    async fn create_watchlist_product(
        &self,
        user_id: &UserId,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<WatchlistProduct, WatchProductError>;

    async fn delete_watchlist_product(
        &self,
        user_id: &UserId,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<(), WatchProductError>;

    async fn update_watchlist_product(
        &self,
        user_id: &UserId,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
        update: UpdateWatchlistProductCommand,
    ) -> Result<WatchlistProduct, WatchProductError>;

    async fn view_watchlist(
        &self,
        user_id: &UserId,
        sort: &Option<Sort<SortWatchlistProductField>>,
        cursor: &Option<Cursor<OffsetDateTime>>,
    ) -> Result<CursoredResult<WatchlistProduct, OffsetDateTime>, WatchProductError>;

    async fn find_user_ids_watching_product(
        &self,
        product_id: &ProductId,
    ) -> Result<Vec<(UserId, bool)>, WatchProductError>;
}

pub struct ProductWatchListServiceImpl<'a> {
    watchlist_repository: &'a (dyn WatchlistProductDynamoDbRepository + Sync),
    product_repository: &'a (dyn ProductDynamoDbRepository + Sync),
    user_service: &'a (dyn UserService + Sync),
}

impl<'a> ProductWatchListServiceImpl<'a> {
    pub fn new(
        watchlist_repository: &'a (dyn WatchlistProductDynamoDbRepository + Sync),
        product_repository: &'a (dyn ProductDynamoDbRepository + Sync),
        user_service: &'a (dyn UserService + Sync),
    ) -> Self {
        Self {
            watchlist_repository,
            product_repository,
            user_service,
        }
    }
}

#[async_trait::async_trait]
impl<'a> ProductWatchListService for ProductWatchListServiceImpl<'a> {
    async fn find_watchlist_product(
        &self,
        user_id: &UserId,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<WatchlistProduct, WatchProductError> {
        let watchlist_record = self
            .watchlist_repository
            .get_watchlist_record(user_id, shop_id, shops_product_id)
            .await?
            .ok_or(WatchProductError::WatchlistProductNotFound(
                *user_id,
                *shop_id,
                shops_product_id.clone(),
            ))?;

        Ok(watchlist_record.into())
    }

    async fn create_watchlist_product(
        &self,
        user_id: &UserId,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<WatchlistProduct, WatchProductError> {
        let user = self
            .user_service
            .find_user(user_id)
            .await
            .map_err(|e| match e {
                UserServiceError::UserNotFound(id) => WatchProductError::UserNotFound(id),
                other => WatchProductError::UserServiceError(other),
            })?;

        let product_record = self
            .product_repository
            .get_product_record(shop_id, shops_product_id)
            .await?
            .ok_or(WatchProductError::ProductNotFound(
                *shop_id,
                shops_product_id.clone(),
            ))?;

        let now = OffsetDateTime::now_utc();

        let limit = user.tier.watchlist_quota();
        let watchlist_count = self
            .watchlist_repository
            .count_watchlist_records(user_id, &Default::default(), true)
            .await?;
        if watchlist_count >= limit as u64 {
            return Err(WatchProductError::WatchlistEntryCountExceeded(
                watchlist_count as u32,
                limit,
            ));
        }

        let watchlist_record = WatchlistProductRecord {
            pk: mk_pk(user_id),
            sk: mk_sk(shop_id, shops_product_id),
            lsi1_sk: mk_lsi1_sk(&now),
            gsi1_pk: mk_gsi1_pk(&product_record.product_id),
            gsi1_sk: mk_gsi1_sk(user_id),
            user_id: *user_id,
            product_id: product_record.product_id,
            shop_id: product_record.shop_id,
            shops_product_id: product_record.shops_product_id,
            notifications: false,
            created: now,
            updated: now,
        };
        self.watchlist_repository
            .put_watchlist_record(watchlist_record.clone())
            .await?;

        Ok(watchlist_record.into())
    }

    async fn delete_watchlist_product(
        &self,
        user_id: &UserId,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<(), WatchProductError> {
        // exists guard
        let _ = self
            .watchlist_repository
            .get_watchlist_record(user_id, shop_id, shops_product_id)
            .await?
            .ok_or(WatchProductError::WatchlistProductNotFound(
                *user_id,
                *shop_id,
                shops_product_id.clone(),
            ))?;

        self.watchlist_repository
            .delete_watchlist_record(user_id, shop_id, shops_product_id)
            .await?;

        Ok(())
    }

    async fn update_watchlist_product(
        &self,
        user_id: &UserId,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
        update: UpdateWatchlistProductCommand,
    ) -> Result<WatchlistProduct, WatchProductError> {
        let watchlist_record = self
            .watchlist_repository
            .get_watchlist_record(user_id, shop_id, shops_product_id)
            .await?
            .ok_or(WatchProductError::WatchlistProductNotFound(
                *user_id,
                *shop_id,
                shops_product_id.clone(),
            ))?;

        if update.is_empty() {
            Ok(watchlist_record.into())
        } else {
            let updated_watchlist_record = self
                .watchlist_repository
                .update_watchlist_record(
                    user_id,
                    shop_id,
                    shops_product_id,
                    WatchlistProductRecordUpdate::from_cmd(update),
                )
                .await?
                .ok_or_else(|| {
                    WatchProductError::SdkUpdateItemError(SdkError::construction_failure(
                        "Failed parsing DynamoDB UpdateItem Response-Payload",
                    ))
                })?;

            Ok(updated_watchlist_record.into())
        }
    }

    async fn view_watchlist(
        &self,
        user_id: &UserId,
        sort: &Option<Sort<SortWatchlistProductField>>,
        cursor: &Option<Cursor<OffsetDateTime>>,
    ) -> Result<CursoredResult<WatchlistProduct, OffsetDateTime>, WatchProductError> {
        let sort = sort.unwrap_or(Sort {
            sort: SortWatchlistProductField::Created,
            order: SortOrder::Asc,
        });
        let cursor = (*cursor).unwrap_or_default();
        let scan_index_forward = matches!(sort.order, SortOrder::Asc);
        let paged_watchlist_records = self
            .watchlist_repository
            .query_watchlist_records(user_id, &cursor, scan_index_forward)
            .await?;
        let last = paged_watchlist_records.last().cloned();
        let products: Vec<WatchlistProduct> = paged_watchlist_records
            .into_iter()
            .map(WatchlistProduct::from)
            .collect();

        let total = if products.is_empty() {
            0
        } else {
            self.watchlist_repository
                .count_watchlist_records(user_id, &cursor, scan_index_forward)
                .await?
        };
        Ok(CursoredResult {
            cursor: Cursor {
                size: products.len() as u64,
                search_after: last.map(|last| last.created),
            },
            items: products,
            total: Some(total),
        })
    }

    async fn find_user_ids_watching_product(
        &self,
        product_id: &ProductId,
    ) -> Result<Vec<(UserId, bool)>, WatchProductError> {
        self.watchlist_repository
            .query_user_ids_watching_product(product_id)
            .await
            .map_err(WatchProductError::from)
    }
}

#[cfg(test)]
mod tests {
    mod find_watchlist_product {
        use crate::{
            dynamodb::repository::MockWatchlistProductDynamoDbRepository,
            service::product_watchlist_service::{
                ProductWatchListService, ProductWatchListServiceImpl, WatchProductError,
            },
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::{shop_id::ShopId, shops_product_id::ShopsProductId, user_id::UserId};
        use fake::{Fake, Faker};
        use product::dynamodb::repository::MockProductDynamoDbRepository;

        #[tokio::test]
        async fn should_err_watchlist_timestamp_not_found_when_no_watched_product_with_timestamp_exists()
         {
            let product_repository = MockProductDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_get_watchlist_record()
                .return_once(|_, _, _| Box::pin(async { Ok(None) }));

            let user_service = user::service::user_service::MockUserService::default();
            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &product_repository,
                &user_service,
            );
            let user_id = UserId::new();
            let shop_id = ShopId::new();
            let shops_product_id = ShopsProductId::new();
            let actual = service
                .find_watchlist_product(&user_id, &shop_id, &shops_product_id)
                .await
                .unwrap_err();

            match actual {
                WatchProductError::WatchlistProductNotFound(
                    err_user_id,
                    err_shop_id,
                    err_shops_product_id,
                ) => {
                    assert_eq!(user_id, err_user_id);
                    assert_eq!(shop_id, err_shop_id);
                    assert_eq!(shops_product_id, err_shops_product_id);
                }
                err => {
                    panic!(
                        "Expected 'WatchProductError::WatchlistTimestampNotFound' but got '{err}'"
                    )
                }
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::get_item::GetItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_get_item(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let product_repository = MockProductDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_get_watchlist_record()
                .return_once(|_, _, _| Box::pin(async { Err(expected) }));

            let user_service = user::service::user_service::MockUserService::default();
            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &product_repository,
                &user_service,
            );

            let actual = service
                .find_watchlist_product(&Faker.fake(), &Faker.fake(), &Faker.fake())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                WatchProductError::SdkGetItemError(_) => {}
                err => panic!("Expected 'WatchProductError::SdkGetItemError', got '{err}'"),
            }
        }
    }

    mod create_watchlist_product {
        use crate::{
            core::quota::WatchlistQuota,
            dynamodb::repository::MockWatchlistProductDynamoDbRepository,
            service::product_watchlist_service::{
                ProductWatchListService, ProductWatchListServiceImpl, WatchProductError,
            },
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
            operation::put_item::PutItemOutput,
        };
        use common::{shop_id::ShopId, shops_product_id::ShopsProductId, user_id::UserId};
        use fake::{Fake, Faker};
        use product::dynamodb::repository::MockProductDynamoDbRepository;
        use user::core::user::User;

        #[tokio::test]
        async fn should_watch_when_success() {
            let mut user_service = user::service::user_service::MockUserService::default();
            user_service
                .expect_find_user()
                .return_once(|_| Box::pin(async { Ok(fake::Fake::fake(&fake::Faker)) }));

            let mut product_repository = MockProductDynamoDbRepository::default();
            product_repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));

            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_count_watchlist_records()
                .return_once(|_, _, _| {
                    Box::pin(async {
                        Ok(user::core::tier::UserTier::Free.watchlist_quota() as u64 - 1)
                    })
                });
            watchlist_repository
                .expect_put_watchlist_record()
                .return_once(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));

            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &product_repository,
                &user_service,
            );
            service
                .create_watchlist_product(&Faker.fake(), &Faker.fake(), &Faker.fake())
                .await
                .unwrap();
        }

        #[tokio::test]
        async fn should_err_user_not_found_when_user_not_exists() {
            let user_id = UserId::new();
            let mut user_service = user::service::user_service::MockUserService::default();
            user_service.expect_find_user().return_once(move |_| {
                Box::pin(async move {
                    Err(user::service::user_service::UserServiceError::UserNotFound(
                        user_id,
                    ))
                })
            });

            let product_repository = MockProductDynamoDbRepository::default();
            let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();

            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &product_repository,
                &user_service,
            );
            let actual = service
                .create_watchlist_product(&user_id, &ShopId::new(), &ShopsProductId::new())
                .await
                .unwrap_err();

            match actual {
                WatchProductError::UserNotFound(err_user_id) => {
                    assert_eq!(user_id, err_user_id);
                }
                err => panic!("Expected 'WatchProductError::UserNotFound' but got '{err}'"),
            }
        }

        #[tokio::test]
        async fn should_err_product_not_found_when_product_not_exists() {
            let mut user_service = user::service::user_service::MockUserService::default();
            user_service
                .expect_find_user()
                .return_once(|_| Box::pin(async { Ok(fake::Fake::fake(&fake::Faker)) }));

            let mut product_repository = MockProductDynamoDbRepository::default();
            product_repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));

            let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();

            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &product_repository,
                &user_service,
            );
            let shop_id = ShopId::new();
            let shops_product_id = ShopsProductId::new();
            let actual = service
                .create_watchlist_product(&Faker.fake(), &shop_id, &shops_product_id)
                .await
                .unwrap_err();

            match actual {
                WatchProductError::ProductNotFound(err_shop_id, err_shops_product_id) => {
                    assert_eq!(shop_id, err_shop_id);
                    assert_eq!(shops_product_id, err_shops_product_id);
                }
                err => panic!("Expected 'WatchProductError::ProductNotFound' but got '{err}'"),
            }
        }

        #[tokio::test]
        async fn should_err_watchlist_quota_exceeded_when_exceeded() {
            let mut user_service = user::service::user_service::MockUserService::default();
            user_service.expect_find_user().return_once(|_| {
                Box::pin(async {
                    let mut user: User = fake::Fake::fake(&fake::Faker);
                    user.tier = user::core::tier::UserTier::Free;
                    Ok(user)
                })
            });

            let mut product_repository = MockProductDynamoDbRepository::default();
            product_repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));

            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_count_watchlist_records()
                .return_once(|_, _, _| {
                    Box::pin(async {
                        Ok(user::core::tier::UserTier::Free.watchlist_quota() as u64)
                    })
                });

            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &product_repository,
                &user_service,
            );
            let shop_id = ShopId::new();
            let shops_product_id = ShopsProductId::new();
            let actual = service
                .create_watchlist_product(&Faker.fake(), &shop_id, &shops_product_id)
                .await
                .unwrap_err();

            match actual {
                WatchProductError::WatchlistEntryCountExceeded(actual_count, actual_limit) => {
                    assert_eq!(
                        user::core::tier::UserTier::Free.watchlist_quota(),
                        actual_count
                    );
                    assert_eq!(
                        user::core::tier::UserTier::Free.watchlist_quota(),
                        actual_limit
                    );
                }
                err => panic!(
                    "Expected 'WatchProductError::WatchlistEntryCountExceeded' but got '{err}'"
                ),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::get_item::GetItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_get_product_record(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut user_service = user::service::user_service::MockUserService::default();
            user_service
                .expect_find_user()
                .return_once(|_| Box::pin(async { Ok(fake::Fake::fake(&fake::Faker)) }));

            let mut product_repository = MockProductDynamoDbRepository::default();
            product_repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));

            let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();

            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &product_repository,
                &user_service,
            );

            let actual = service
                .create_watchlist_product(&Faker.fake(), &Faker.fake(), &Faker.fake())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                WatchProductError::SdkGetItemError(_) => {}
                err => panic!("Expected 'WatchProductError::SdkGetItemError', got '{err}'"),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::put_item::PutItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_put_item(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::put_item::PutItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut user_service = user::service::user_service::MockUserService::default();
            user_service
                .expect_find_user()
                .return_once(|_| Box::pin(async { Ok(fake::Fake::fake(&fake::Faker)) }));

            let mut product_repository = MockProductDynamoDbRepository::default();
            product_repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));

            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_count_watchlist_records()
                .return_once(|_, _, _| {
                    Box::pin(async {
                        Ok(fake::rand::random_range(
                            0..user::core::tier::UserTier::Free.watchlist_quota() as u64,
                        ))
                    })
                });
            watchlist_repository
                .expect_put_watchlist_record()
                .return_once(|_| Box::pin(async { Err(expected) }));

            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &product_repository,
                &user_service,
            );

            let actual = service
                .create_watchlist_product(&Faker.fake(), &Faker.fake(), &Faker.fake())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                WatchProductError::SdkPutItemError(_) => {}
                err => panic!("Expected 'WatchProductError::SdkPutItemError', got '{err}'"),
            }
        }
    }

    mod unwatch {
        use crate::{
            dynamodb::repository::MockWatchlistProductDynamoDbRepository,
            service::product_watchlist_service::{
                ProductWatchListService, ProductWatchListServiceImpl, WatchProductError,
            },
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
            operation::delete_item::DeleteItemOutput,
        };
        use common::{shop_id::ShopId, shops_product_id::ShopsProductId, user_id::UserId};
        use fake::{Fake, Faker};
        use product::dynamodb::repository::MockProductDynamoDbRepository;

        #[tokio::test]
        async fn should_unwatch_when_success() {
            let product_repository = MockProductDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_get_watchlist_record()
                .return_once(|_, _, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            watchlist_repository
                .expect_delete_watchlist_record()
                .return_once(|_, _, _| Box::pin(async { Ok(DeleteItemOutput::builder().build()) }));

            let user_service = user::service::user_service::MockUserService::default();
            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &product_repository,
                &user_service,
            );
            service
                .delete_watchlist_product(&Faker.fake(), &Faker.fake(), &Faker.fake())
                .await
                .unwrap();
        }

        #[tokio::test]
        async fn should_err_watchlist_timestamp_not_found_when_no_watched_product_with_timestamp_exists()
         {
            let product_repository = MockProductDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_get_watchlist_record()
                .return_once(|_, _, _| Box::pin(async { Ok(None) }));

            let user_service = user::service::user_service::MockUserService::default();
            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &product_repository,
                &user_service,
            );
            let user_id = UserId::new();
            let shop_id = ShopId::new();
            let shops_product_id = ShopsProductId::new();
            let actual = service
                .delete_watchlist_product(&user_id, &shop_id, &shops_product_id)
                .await
                .unwrap_err();

            match actual {
                WatchProductError::WatchlistProductNotFound(
                    err_user_id,
                    err_shop_id,
                    err_shops_product_id,
                ) => {
                    assert_eq!(user_id, err_user_id);
                    assert_eq!(shop_id, err_shop_id);
                    assert_eq!(shops_product_id, err_shops_product_id);
                }
                err => {
                    panic!(
                        "Expected 'WatchProductError::WatchlistTimestampNotFound' but got '{err}'"
                    )
                }
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::get_item::GetItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_get_item(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let product_repository = MockProductDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_get_watchlist_record()
                .return_once(|_, _, _| Box::pin(async { Err(expected) }));

            let user_service = user::service::user_service::MockUserService::default();
            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &product_repository,
                &user_service,
            );

            let actual = service
                .delete_watchlist_product(&Faker.fake(), &Faker.fake(), &Faker.fake())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                WatchProductError::SdkGetItemError(_) => {}
                err => panic!("Expected 'WatchProductError::SdkGetItemError', got '{err}'"),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::delete_item::DeleteItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_delete_item(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::delete_item::DeleteItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let product_repository = MockProductDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_get_watchlist_record()
                .return_once(|_, _, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            watchlist_repository
                .expect_delete_watchlist_record()
                .return_once(|_, _, _| Box::pin(async { Err(expected) }));

            let user_service = user::service::user_service::MockUserService::default();
            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &product_repository,
                &user_service,
            );

            let actual = service
                .delete_watchlist_product(&Faker.fake(), &Faker.fake(), &Faker.fake())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                WatchProductError::SdkDeleteItemError(_) => {}
                err => panic!("Expected 'WatchProductError::SdkDeleteItemError', got '{err}'"),
            }
        }
    }

    mod toggle_notifications {
        use crate::{
            dynamodb::record::WatchlistProductRecord,
            dynamodb::repository::MockWatchlistProductDynamoDbRepository,
            service::command::UpdateWatchlistProductCommand,
            service::product_watchlist_service::{
                ProductWatchListService, ProductWatchListServiceImpl, WatchProductError,
            },
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::{shop_id::ShopId, shops_product_id::ShopsProductId, user_id::UserId};
        use fake::{Fake, Faker};
        use product::dynamodb::repository::MockProductDynamoDbRepository;

        #[tokio::test]
        async fn should_toggle_notifications_when_success() {
            let product_repository = MockProductDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_get_watchlist_record()
                .return_once(|_, _, _| {
                    Box::pin(async {
                        let mut faked = Faker.fake::<WatchlistProductRecord>();
                        faked.notifications = false;
                        Ok(Some(faked))
                    })
                });
            watchlist_repository
                .expect_update_watchlist_record()
                .return_once(|_, _, _, _| Box::pin(async { Ok(Some(Faker.fake())) }));

            let user_service = user::service::user_service::MockUserService::default();
            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &product_repository,
                &user_service,
            );
            service
                .update_watchlist_product(
                    &Faker.fake(),
                    &Faker.fake(),
                    &Faker.fake(),
                    UpdateWatchlistProductCommand {
                        notifications: Some(true),
                    },
                )
                .await
                .unwrap();
        }

        #[tokio::test]
        async fn should_err_watchlist_timestamp_not_found_when_no_watched_product_with_timestamp_exists()
         {
            let product_repository = MockProductDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_get_watchlist_record()
                .return_once(|_, _, _| Box::pin(async { Ok(None) }));

            let user_service = user::service::user_service::MockUserService::default();
            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &product_repository,
                &user_service,
            );
            let user_id = UserId::new();
            let shop_id = ShopId::new();
            let shops_product_id = ShopsProductId::new();
            let actual = service
                .update_watchlist_product(
                    &user_id,
                    &shop_id,
                    &shops_product_id,
                    UpdateWatchlistProductCommand {
                        notifications: Some(true),
                    },
                )
                .await
                .unwrap_err();

            match actual {
                WatchProductError::WatchlistProductNotFound(
                    err_user_id,
                    err_shop_id,
                    err_shops_product_id,
                ) => {
                    assert_eq!(user_id, err_user_id);
                    assert_eq!(shop_id, err_shop_id);
                    assert_eq!(shops_product_id, err_shops_product_id);
                }
                err => {
                    panic!(
                        "Expected 'WatchProductError::WatchlistTimestampNotFound' but got '{err}'"
                    )
                }
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::get_item::GetItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_get_item(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let product_repository = MockProductDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_get_watchlist_record()
                .return_once(|_, _, _| Box::pin(async { Err(expected) }));

            let user_service = user::service::user_service::MockUserService::default();
            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &product_repository,
                &user_service,
            );

            let actual = service
                .update_watchlist_product(
                    &Faker.fake(),
                    &Faker.fake(),
                    &Faker.fake(),
                    UpdateWatchlistProductCommand {
                        notifications: Some(true),
                    },
                )
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                WatchProductError::SdkGetItemError(_) => {}
                err => panic!("Expected 'WatchProductError::SdkGetItemError', got '{err}'"),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::update_item::UpdateItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_update_item(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::update_item::UpdateItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let product_repository = MockProductDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_get_watchlist_record()
                .return_once(|_, _, _| {
                    Box::pin(async {
                        let mut faked = Faker.fake::<WatchlistProductRecord>();
                        faked.notifications = true;
                        Ok(Some(faked))
                    })
                });
            watchlist_repository
                .expect_update_watchlist_record()
                .return_once(|_, _, _, _| Box::pin(async { Err(expected) }));

            let user_service = user::service::user_service::MockUserService::default();
            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &product_repository,
                &user_service,
            );

            let actual = service
                .update_watchlist_product(
                    &Faker.fake(),
                    &Faker.fake(),
                    &Faker.fake(),
                    UpdateWatchlistProductCommand {
                        notifications: Some(true),
                    },
                )
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                WatchProductError::SdkUpdateItemError(_) => {}
                err => panic!("Expected 'WatchProductError::SdkUpdateItemError', got '{err}'"),
            }
        }
    }

    mod view_watchlist {
        use crate::{
            dynamodb::repository::MockWatchlistProductDynamoDbRepository,
            service::product_watchlist_service::{
                ProductWatchListService, ProductWatchListServiceImpl, WatchProductError,
            },
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use fake::{Fake, Faker};
        use product::dynamodb::repository::MockProductDynamoDbRepository;

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::query::QueryError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_query(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::query::QueryError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let product_repository = MockProductDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_query_watchlist_records()
                .return_once(|_, _, _| Box::pin(async { Err(expected) }));
            let user_service = user::service::user_service::MockUserService::default();
            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &product_repository,
                &user_service,
            );
            let actual = service.view_watchlist(&Faker.fake(), &None, &None).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                WatchProductError::SdkQueryError(_) => {}
                err => panic!("Expected 'WatchProductError::SdkQueryError', got '{err}'"),
            }
        }
    }
}
