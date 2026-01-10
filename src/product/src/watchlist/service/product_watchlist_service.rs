use crate::dynamodb::repository::ProductDynamoDbRepository;
use crate::service::get_service::{GetProductError, GetProductService};
use crate::{
    watchlist::core::watchlist_product::{LocalizedWatchlistProductView, WatchlistProduct},
    watchlist::dynamodb::record::{WatchlistProductRecord, mk_lsi1_sk, mk_pk, mk_sk},
    watchlist::dynamodb::record_update::WatchlistProductRecordUpdate,
    watchlist::dynamodb::repository::WatchlistProductDynamoDbRepository,
    watchlist::service::command::UpdateWatchlistProductCommand,
    watchlist::service::sort_watchlist_product_field::SortWatchlistProductField,
};
use aws_sdk_dynamodb::{
    config::http::HttpResponse, error::SdkError, operation::put_item::PutItemError,
};
use common::{
    currency::domain::Currency,
    language::domain::Language,
    pagination::cursor::{Cursor, CursoredResult},
    price::domain::MonetaryAmountOverflowError,
    product_id::{ProductId, ProductKey},
    shop_id::ShopId,
    shops_product_id::ShopsProductId,
    sort::{Sort, SortOrder},
    user_id::UserId,
};
use std::collections::HashMap;
use time::OffsetDateTime;
use user::core::user::User;
use user::dynamodb::repository::UserDynamoDbRepository;

pub const MAX_WATCHLIST_QUOTA: usize = 5;

#[derive(thiserror::Error, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum WatchProductError {
    #[error("{0}")]
    MonetaryAmountOverflowError(#[from] MonetaryAmountOverflowError),

    #[error("Product with ShopId '{0}' and ShopsProductId '{1}' not found.")]
    ProductNotFound(ShopId, ShopsProductId),

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
        "Exceeded the maximum amount of watchlist entries. There are already {0}/{MAX_WATCHLIST_QUOTA} watchlist entries occupied."
    )]
    WatchlistEntryCountExceeded(u32),
}

impl From<GetProductError> for WatchProductError {
    fn from(err: GetProductError) -> Self {
        match err {
            GetProductError::ProductNotFound(shop_id, shops_product_id) => {
                WatchProductError::ProductNotFound(shop_id, shops_product_id)
            }
            GetProductError::MonetaryAmountOverflowError(e) => {
                WatchProductError::MonetaryAmountOverflowError(e)
            }
            GetProductError::SdkGetItemError(e) => WatchProductError::SdkGetItemError(e),
            GetProductError::SdkBatchGetItemError(e) => WatchProductError::SdkBatchGetItemError(e),
            GetProductError::SdkQueryError(e) => WatchProductError::SdkQueryError(e),
            GetProductError::UnprocessedAfterMaxRetries(e) => {
                WatchProductError::UnprocessedAfterMaxRetries(e)
            }
        }
    }
}

#[cfg(feature = "data")]
pub mod api {
    use crate::watchlist::service::product_watchlist_service::WatchProductError;
    use common::api::error::ApiError;
    use common::api::error_code::{
        MONETARY_AMOUNT_OVERFLOW, PRODUCT_NOT_FOUND, UNPROCESSED_AFTER_MAX_RETRIES, USER_NOT_FOUND,
        WATCHLIST_ENTRY_NOT_FOUND, WATCHLIST_QUOTA_EXCEEDED,
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
                WatchProductError::UserNotFound(_) => {
                    ApiError::internal_server_error(USER_NOT_FOUND, Box::new(err))
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
                WatchProductError::WatchlistEntryCountExceeded(_) => {
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
        languages: &[Language],
        currency: &Currency,
        sort: &Option<Sort<SortWatchlistProductField>>,
        cursor: &Option<Cursor<OffsetDateTime>>,
    ) -> Result<CursoredResult<LocalizedWatchlistProductView, OffsetDateTime>, WatchProductError>;

    async fn find_users_with_notifications(
        &self,
        product_id: &ProductId,
    ) -> Result<Vec<User>, WatchProductError>;
}

pub struct ProductWatchListServiceImpl<'a> {
    watchlist_repository: &'a (dyn WatchlistProductDynamoDbRepository + Sync),
    user_repository: &'a (dyn UserDynamoDbRepository + Sync),
    product_repository: &'a (dyn ProductDynamoDbRepository + Sync),
    get_product_service: &'a (dyn GetProductService + Sync),
}

impl<'a> ProductWatchListServiceImpl<'a> {
    pub fn new(
        watchlist_repository: &'a (dyn WatchlistProductDynamoDbRepository + Sync),
        user_repository: &'a (dyn UserDynamoDbRepository + Sync),
        product_repository: &'a (dyn ProductDynamoDbRepository + Sync),
        get_product_service: &'a (dyn GetProductService + Sync),
    ) -> Self {
        Self {
            watchlist_repository,
            user_repository,
            product_repository,
            get_product_service,
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
        let product_record = self
            .product_repository
            .get_product_record(shop_id, shops_product_id)
            .await?
            .ok_or(WatchProductError::ProductNotFound(
                *shop_id,
                shops_product_id.clone(),
            ))?;

        let now = OffsetDateTime::now_utc();
        let user_record = self
            .user_repository
            .get_user_record(user_id)
            .await?
            .ok_or(WatchProductError::UserNotFound(*user_id))?;

        let watchlist_count = self
            .watchlist_repository
            .count_watchlist_records(user_id, &Default::default(), true)
            .await?;
        if watchlist_count as usize >= MAX_WATCHLIST_QUOTA {
            return Err(WatchProductError::WatchlistEntryCountExceeded(
                watchlist_count as u32,
            ));
        }

        let watchlist_record = WatchlistProductRecord {
            pk: mk_pk(user_id),
            sk: mk_sk(shop_id, shops_product_id),
            lsi1_sk: mk_lsi1_sk(&now)
                .map_err::<SdkError<PutItemError>, _>(SdkError::construction_failure)?,
            gsi1_pk: None,
            gsi1_sk: None,
            user_id: *user_id,
            product_id: product_record.product_id,
            shop_id: product_record.shop_id,
            shops_product_id: product_record.shops_product_id,
            notifications: false,
            user_record,
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
                    WatchlistProductRecordUpdate::from_cmd(
                        update,
                        user_id,
                        &watchlist_record.product_id,
                    ),
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
        languages: &[Language],
        currency: &Currency,
        sort: &Option<Sort<SortWatchlistProductField>>,
        cursor: &Option<Cursor<OffsetDateTime>>,
    ) -> Result<CursoredResult<LocalizedWatchlistProductView, OffsetDateTime>, WatchProductError>
    {
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

        let mut watchlist_records_created = paged_watchlist_records
            .iter()
            .map(|record| (record.product_id, record.clone()))
            .collect::<HashMap<_, _>>();
        let watchlist_record_keys = paged_watchlist_records
            .into_iter()
            .map(|record| ProductKey::new(record.shop_id, record.shops_product_id))
            .collect();
        let mut products = self
                    .get_product_service
                    .view_products(watchlist_record_keys, languages, currency)
                    .await?
                    .into_iter()
                    .filter_map(
                        |item| match watchlist_records_created.remove(&item.product_id) {
                            Some(watchlist_record) => Some(LocalizedWatchlistProductView {
                                product: item,
                                notifications: watchlist_record.notifications,
                                created: watchlist_record.created,
                                updated: watchlist_record.updated,
                            }),
                            None => {
                                tracing::error!("Could not find timestamp 'created' for Watchlist-Product after Batch-Get. This is a bug. Skipping Product.");
                                None
                            },
                        },
                    )
                    .collect::<Vec<_>>();
        // BatchGetItem responds with any order, so we need to restore the order from the query manually
        products.sort_by(|l, r| {
            if scan_index_forward {
                l.created.cmp(&r.created)
            } else {
                l.created.cmp(&r.created).reverse()
            }
        });

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

    async fn find_users_with_notifications(
        &self,
        product_id: &ProductId,
    ) -> Result<Vec<User>, WatchProductError> {
        let users = self
            .watchlist_repository
            .query_user_records_with_notifications(product_id)
            .await?
            .into_iter()
            .map(User::from)
            .collect();

        Ok(users)
    }
}

#[cfg(test)]
mod tests {
    mod find_watchlist_product {
        use crate::dynamodb::repository::MockProductDynamoDbRepository;
        use crate::service::get_service::GetProductServiceImpl;
        use crate::{
            watchlist::dynamodb::repository::MockWatchlistProductDynamoDbRepository,
            watchlist::service::product_watchlist_service::{
                ProductWatchListService, ProductWatchListServiceImpl, WatchProductError,
            },
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::{shop_id::ShopId, shops_product_id::ShopsProductId, user_id::UserId};
        use fake::{Fake, Faker};
        use user::dynamodb::repository::MockUserDynamoDbRepository;

        #[tokio::test]
        async fn should_err_watchlist_timestamp_not_found_when_no_watched_product_with_timestamp_exists()
         {
            let user_repository = MockUserDynamoDbRepository::default();
            let product_repository = MockProductDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_get_watchlist_record()
                .return_once(|_, _, _| Box::pin(async { Ok(None) }));
            let get_product_service = GetProductServiceImpl::new(&product_repository);

            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &user_repository,
                &product_repository,
                &get_product_service,
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
            let user_repository = MockUserDynamoDbRepository::default();
            let product_repository = MockProductDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_get_watchlist_record()
                .return_once(|_, _, _| Box::pin(async { Err(expected) }));
            let get_product_service = GetProductServiceImpl::new(&product_repository);

            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &user_repository,
                &product_repository,
                &get_product_service,
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
        use crate::dynamodb::repository::MockProductDynamoDbRepository;
        use crate::service::get_service::GetProductServiceImpl;
        use crate::watchlist::service::product_watchlist_service::MAX_WATCHLIST_QUOTA;
        use crate::{
            watchlist::dynamodb::repository::MockWatchlistProductDynamoDbRepository,
            watchlist::service::product_watchlist_service::{
                ProductWatchListService, ProductWatchListServiceImpl, WatchProductError,
            },
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
            operation::put_item::PutItemOutput,
        };
        use common::{shop_id::ShopId, shops_product_id::ShopsProductId};
        use fake::{Fake, Faker};
        use user::dynamodb::repository::MockUserDynamoDbRepository;

        #[tokio::test]
        async fn should_watch_when_success() {
            let mut user_repository = MockUserDynamoDbRepository::default();
            user_repository
                .expect_get_user_record()
                .return_once(|_| Box::pin(async { Ok(Some(Faker.fake())) }));
            let mut product_repository = MockProductDynamoDbRepository::default();
            product_repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));

            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_count_watchlist_records()
                .return_once(|_, _, _| Box::pin(async { Ok(MAX_WATCHLIST_QUOTA as u64 - 1) }));
            watchlist_repository
                .expect_put_watchlist_record()
                .return_once(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));
            let get_product_service = GetProductServiceImpl::new(&product_repository);

            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &user_repository,
                &product_repository,
                &get_product_service,
            );
            service
                .create_watchlist_product(&Faker.fake(), &Faker.fake(), &Faker.fake())
                .await
                .unwrap();
        }

        #[tokio::test]
        async fn should_err_product_not_found_when_product_not_exists() {
            let user_repository = MockUserDynamoDbRepository::default();
            let mut product_repository = MockProductDynamoDbRepository::default();
            product_repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));

            let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            let get_product_service = GetProductServiceImpl::new(&product_repository);

            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &user_repository,
                &product_repository,
                &get_product_service,
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
            let mut user_repository = MockUserDynamoDbRepository::default();
            user_repository
                .expect_get_user_record()
                .return_once(|_| Box::pin(async { Ok(Some(Faker.fake())) }));
            let mut product_repository = MockProductDynamoDbRepository::default();
            product_repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));

            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_count_watchlist_records()
                .return_once(|_, _, _| Box::pin(async { Ok(MAX_WATCHLIST_QUOTA as u64) }));

            let get_product_service = GetProductServiceImpl::new(&product_repository);

            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &user_repository,
                &product_repository,
                &get_product_service,
            );
            let shop_id = ShopId::new();
            let shops_product_id = ShopsProductId::new();
            let actual = service
                .create_watchlist_product(&Faker.fake(), &shop_id, &shops_product_id)
                .await
                .unwrap_err();

            match actual {
                WatchProductError::WatchlistEntryCountExceeded(actual_count) => {
                    assert_eq!(MAX_WATCHLIST_QUOTA, actual_count as usize);
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
            let user_repository = MockUserDynamoDbRepository::default();
            let mut product_repository = MockProductDynamoDbRepository::default();
            product_repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));

            let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            let get_product_service = GetProductServiceImpl::new(&product_repository);

            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &user_repository,
                &product_repository,
                &get_product_service,
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
            aws_sdk_dynamodb::operation::get_item::GetItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_get_user_record(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut user_repository = MockUserDynamoDbRepository::default();
            user_repository
                .expect_get_user_record()
                .return_once(|_| Box::pin(async { Err(expected) }));
            let mut product_repository = MockProductDynamoDbRepository::default();
            product_repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));

            let watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            let get_product_service = GetProductServiceImpl::new(&product_repository);

            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &user_repository,
                &product_repository,
                &get_product_service,
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
            let mut user_repository = MockUserDynamoDbRepository::default();
            user_repository
                .expect_get_user_record()
                .return_once(|_| Box::pin(async { Ok(Some(Faker.fake())) }));
            let mut product_repository = MockProductDynamoDbRepository::default();
            product_repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));

            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_count_watchlist_records()
                .return_once(|_, _, _| {
                    Box::pin(async { Ok(fake::rand::random_range(0..MAX_WATCHLIST_QUOTA as u64)) })
                });
            watchlist_repository
                .expect_put_watchlist_record()
                .return_once(|_| Box::pin(async { Err(expected) }));
            let get_product_service = GetProductServiceImpl::new(&product_repository);

            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &user_repository,
                &product_repository,
                &get_product_service,
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
        use crate::dynamodb::repository::MockProductDynamoDbRepository;
        use crate::service::get_service::GetProductServiceImpl;
        use crate::{
            watchlist::dynamodb::repository::MockWatchlistProductDynamoDbRepository,
            watchlist::service::product_watchlist_service::{
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
        use user::dynamodb::repository::MockUserDynamoDbRepository;

        #[tokio::test]
        async fn should_unwatch_when_success() {
            let user_repository = MockUserDynamoDbRepository::default();
            let product_repository = MockProductDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_get_watchlist_record()
                .return_once(|_, _, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            watchlist_repository
                .expect_delete_watchlist_record()
                .return_once(|_, _, _| Box::pin(async { Ok(DeleteItemOutput::builder().build()) }));
            let get_product_service = GetProductServiceImpl::new(&product_repository);

            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &user_repository,
                &product_repository,
                &get_product_service,
            );
            service
                .delete_watchlist_product(&Faker.fake(), &Faker.fake(), &Faker.fake())
                .await
                .unwrap();
        }

        #[tokio::test]
        async fn should_err_watchlist_timestamp_not_found_when_no_watched_product_with_timestamp_exists()
         {
            let user_repository = MockUserDynamoDbRepository::default();
            let product_repository = MockProductDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_get_watchlist_record()
                .return_once(|_, _, _| Box::pin(async { Ok(None) }));
            let get_product_service = GetProductServiceImpl::new(&product_repository);

            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &user_repository,
                &product_repository,
                &get_product_service,
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
            let user_repository = MockUserDynamoDbRepository::default();
            let product_repository = MockProductDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_get_watchlist_record()
                .return_once(|_, _, _| Box::pin(async { Err(expected) }));
            let get_product_service = GetProductServiceImpl::new(&product_repository);

            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &user_repository,
                &product_repository,
                &get_product_service,
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
            let user_repository = MockUserDynamoDbRepository::default();
            let product_repository = MockProductDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_get_watchlist_record()
                .return_once(|_, _, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            watchlist_repository
                .expect_delete_watchlist_record()
                .return_once(|_, _, _| Box::pin(async { Err(expected) }));
            let get_product_service = GetProductServiceImpl::new(&product_repository);

            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &user_repository,
                &product_repository,
                &get_product_service,
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
        use crate::dynamodb::repository::MockProductDynamoDbRepository;
        use crate::service::get_service::GetProductServiceImpl;
        use crate::{
            watchlist::dynamodb::record::WatchlistProductRecord,
            watchlist::dynamodb::repository::MockWatchlistProductDynamoDbRepository,
            watchlist::service::command::UpdateWatchlistProductCommand,
            watchlist::service::product_watchlist_service::{
                ProductWatchListService, ProductWatchListServiceImpl, WatchProductError,
            },
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::{shop_id::ShopId, shops_product_id::ShopsProductId, user_id::UserId};
        use fake::{Fake, Faker};
        use user::dynamodb::repository::MockUserDynamoDbRepository;

        #[tokio::test]
        async fn should_toggle_notifications_when_success() {
            let user_repository = MockUserDynamoDbRepository::default();
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
            let get_product_service = GetProductServiceImpl::new(&product_repository);

            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &user_repository,
                &product_repository,
                &get_product_service,
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
            let user_repository = MockUserDynamoDbRepository::default();
            let product_repository = MockProductDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_get_watchlist_record()
                .return_once(|_, _, _| Box::pin(async { Ok(None) }));
            let get_product_service = GetProductServiceImpl::new(&product_repository);

            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &user_repository,
                &product_repository,
                &get_product_service,
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
                        notifications: Some(false),
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
            let user_repository = MockUserDynamoDbRepository::default();
            let product_repository = MockProductDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_get_watchlist_record()
                .return_once(|_, _, _| Box::pin(async { Err(expected) }));
            let get_product_service = GetProductServiceImpl::new(&product_repository);

            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &user_repository,
                &product_repository,
                &get_product_service,
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
            let user_repository = MockUserDynamoDbRepository::default();
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
            let get_product_service = GetProductServiceImpl::new(&product_repository);

            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &user_repository,
                &product_repository,
                &get_product_service,
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
        use crate::dynamodb::repository::MockProductDynamoDbRepository;
        use crate::service::get_service::MockGetProductService;
        use crate::{
            watchlist::dynamodb::repository::MockWatchlistProductDynamoDbRepository,
            watchlist::service::product_watchlist_service::{
                ProductWatchListService, ProductWatchListServiceImpl, WatchProductError,
            },
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::{currency::domain::Currency, language::domain::Language};
        use fake::{Fake, Faker};
        use user::dynamodb::repository::MockUserDynamoDbRepository;

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
            let user_repository = MockUserDynamoDbRepository::default();
            let product_repository = MockProductDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::default();
            watchlist_repository
                .expect_query_watchlist_records()
                .return_once(|_, _, _| Box::pin(async { Err(expected) }));
            let get_product_service = MockGetProductService::default();
            let service = ProductWatchListServiceImpl::new(
                &watchlist_repository,
                &user_repository,
                &product_repository,
                &get_product_service,
            );
            let actual = service
                .view_watchlist(&Faker.fake(), &[Language::De], &Currency::Eur, &None, &None)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                WatchProductError::SdkQueryError(_) => {}
                err => panic!("Expected 'WatchProductError::SdkQueryError', got '{err}'"),
            }
        }
    }
}
