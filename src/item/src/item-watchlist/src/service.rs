use crate::{
    domain::LocalizedWatchlistItemView,
    record::{WatchlistItemRecord, mk_pk, mk_sk},
    repository::WatchlistItemDynamoDbRepository,
    sort_watch_item::SortWatchlistItemField,
};
use aws_sdk_dynamodb::{
    config::http::HttpResponse, error::SdkError, operation::put_item::PutItemError,
};
use common::{
    currency::domain::Currency,
    item_id::ItemKey,
    language::domain::Language,
    page::Page,
    paginated_result::PaginatedResult,
    price::domain::MonetaryAmountOverflowError,
    shop_id::ShopId,
    shops_item_id::ShopsItemId,
    sort::{Sort, SortOrder},
    user_id::UserId,
};
use item_dynamodb::repository::ItemDynamoDbRepository;
use item_service::get_service::{GetItemError, GetItemService};
use std::collections::HashMap;
use time::{OffsetDateTime, macros::datetime};

#[derive(thiserror::Error, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum WatchItemError {
    #[error("{0}")]
    MonetaryAmountOverflowError(#[from] MonetaryAmountOverflowError),

    #[error("Item with ShopId '{0}' and ShopsItemId '{1}' not found.")]
    ItemNotFound(ShopId, ShopsItemId),

    #[error(
        "There exists no Watchlist-Item that was started being watched on '{1}' for user '{0}'."
    )]
    WatchlistTimestampNotFound(UserId, OffsetDateTime),

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
}

impl From<GetItemError> for WatchItemError {
    fn from(err: GetItemError) -> Self {
        match err {
            GetItemError::ItemNotFound(shop_id, shops_item_id) => {
                WatchItemError::ItemNotFound(shop_id, shops_item_id)
            }
            GetItemError::MonetaryAmountOverflowError(e) => {
                WatchItemError::MonetaryAmountOverflowError(e)
            }
            GetItemError::SdkGetItemError(e) => WatchItemError::SdkGetItemError(e),
            GetItemError::SdkBatchGetItemError(e) => WatchItemError::SdkBatchGetItemError(e),
            GetItemError::SdkQueryError(e) => WatchItemError::SdkQueryError(e),
        }
    }
}

#[cfg(feature = "api")]
pub mod api {
    use crate::service::WatchItemError;
    use common::api::error::ApiError;
    use common::api::error_code::{
        ITEM_NOT_FOUND, MONETARY_AMOUNT_OVERFLOW, WATCHLIST_ENTRY_NOT_FOUND,
    };
    use tracing::error;

    impl From<WatchItemError> for ApiError {
        fn from(err: WatchItemError) -> Self {
            match err {
                WatchItemError::MonetaryAmountOverflowError(err) => {
                    error!(error = %err, "Encountered MonetaryAmountOverflowError while getting item.");
                    ApiError::internal_server_error(MONETARY_AMOUNT_OVERFLOW)
                }
                WatchItemError::ItemNotFound(_, _) => ApiError::not_found(ITEM_NOT_FOUND),
                WatchItemError::WatchlistTimestampNotFound(_, _) => {
                    ApiError::not_found(WATCHLIST_ENTRY_NOT_FOUND)
                }
                WatchItemError::SdkGetItemError(err) => {
                    error!(error = ?err, "Encountered SdkGetItemError while getting item.");
                    err.into()
                }
                WatchItemError::SdkBatchGetItemError(err) => {
                    error!(error = ?err, "Encountered SdkBatchGetItemError while getting item.");
                    err.into()
                }
                WatchItemError::SdkQueryError(err) => {
                    error!(error = ?err, "Encountered SdkQueryError while querying watchlist.");
                    err.into()
                }
                WatchItemError::SdkPutItemError(err) => {
                    error!(error = ?err, "Encountered SdkPutItemError while writing to watchlist.");
                    err.into()
                }
                WatchItemError::SdkDeleteItemError(err) => {
                    error!(error = ?err, "Encountered SdkPutItemError while deleting item from watchlist.");
                    err.into()
                }
            }
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait ItemWatchListService {
    async fn watch(
        &self,
        user_id: &UserId,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
    ) -> Result<(), WatchItemError>;

    async fn unwatch(
        &self,
        user_id: &UserId,
        created: &OffsetDateTime,
    ) -> Result<(), WatchItemError>;

    async fn view_watchlist(
        &self,
        user_id: &UserId,
        languages: &[Language],
        currency: &Currency,
        sort: &Option<Sort<SortWatchlistItemField>>,
        page: &Option<Page<OffsetDateTime>>,
    ) -> Result<PaginatedResult<LocalizedWatchlistItemView, OffsetDateTime>, WatchItemError>;
}

pub struct ItemWatchListServiceImpl<'a> {
    watchlist_repository: &'a (dyn WatchlistItemDynamoDbRepository + Sync),
    item_repository: &'a (dyn ItemDynamoDbRepository + Sync),
    get_item_service: &'a (dyn GetItemService + Sync),
}

impl<'a> ItemWatchListServiceImpl<'a> {
    pub fn new(
        watchlist_repository: &'a (dyn WatchlistItemDynamoDbRepository + Sync),
        item_repository: &'a (dyn ItemDynamoDbRepository + Sync),
        get_item_service: &'a (dyn GetItemService + Sync),
    ) -> Self {
        Self {
            watchlist_repository,
            item_repository,
            get_item_service,
        }
    }
}

#[async_trait::async_trait]
impl<'a> ItemWatchListService for ItemWatchListServiceImpl<'a> {
    async fn watch(
        &self,
        user_id: &UserId,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
    ) -> Result<(), WatchItemError> {
        let item_record = self
            .item_repository
            .get_item_record(shop_id, shops_item_id)
            .await?
            .ok_or(WatchItemError::ItemNotFound(
                *shop_id,
                shops_item_id.clone(),
            ))?;

        let created = OffsetDateTime::now_utc();
        let watchlist_record = WatchlistItemRecord {
            pk: mk_pk(user_id),
            sk: mk_sk(&created)
                .map_err::<SdkError<PutItemError>, _>(SdkError::construction_failure)?,
            user_id: *user_id,
            item_id: item_record.item_id,
            shop_id: item_record.shop_id,
            shops_item_id: item_record.shops_item_id,
            created,
        };
        self.watchlist_repository
            .put_watchlist_record(watchlist_record)
            .await?;

        Ok(())
    }

    async fn unwatch(
        &self,
        user_id: &UserId,
        created: &OffsetDateTime,
    ) -> Result<(), WatchItemError> {
        // exists guard
        let _ = self
            .watchlist_repository
            .get_watchlist_record(user_id, created)
            .await?
            .ok_or(WatchItemError::WatchlistTimestampNotFound(
                *user_id, *created,
            ))?;

        self.watchlist_repository
            .delete_watchlist_record(user_id, created)
            .await?;

        Ok(())
    }

    async fn view_watchlist(
        &self,
        user_id: &UserId,
        languages: &[Language],
        currency: &Currency,
        sort: &Option<Sort<SortWatchlistItemField>>,
        page: &Option<Page<OffsetDateTime>>,
    ) -> Result<PaginatedResult<LocalizedWatchlistItemView, OffsetDateTime>, WatchItemError> {
        let sort = sort.unwrap_or(Sort {
            sort: SortWatchlistItemField::Created,
            order: SortOrder::Asc,
        });
        let created_guard = page.map(|page| page.from);
        let limit = page.map(|page| page.size.min(100)).unwrap_or(21);
        let scan_index_forward = matches!(sort.order, SortOrder::Asc);
        let paged_watchlist_records = self
            .watchlist_repository
            .query_watchlist_records(user_id, &created_guard, limit, scan_index_forward)
            .await?;

        let next_guard = if scan_index_forward {
            paged_watchlist_records.last().cloned()
        } else {
            paged_watchlist_records.first().cloned()
        };

        let default_guard = if scan_index_forward {
            datetime!(2000 - 01 - 01 0:00 UTC)
        } else {
            OffsetDateTime::now_utc()
        };
        match next_guard {
            None => Ok(PaginatedResult {
                items: vec![],
                page: Page {
                    from: default_guard,
                    size: 0,
                },
                total: None,
                next_after: None,
            }),
            Some(next) => {
                let mut watchlist_records_created = paged_watchlist_records
                    .iter()
                    .map(|record| (record.item_id, record.created))
                    .collect::<HashMap<_, _>>();
                let watchlist_record_keys = paged_watchlist_records
                    .into_iter()
                    .map(|record| ItemKey::new(record.shop_id, record.shops_item_id))
                    .collect();
                let mut items = self
                    .get_item_service
                    .view_items(watchlist_record_keys, languages, currency)
                    .await?
                    .into_iter()
                    .filter_map(
                        |item| match watchlist_records_created.remove(&item.item_id) {
                            Some(created) => Some(LocalizedWatchlistItemView { item, created }),
                            None => {
                                tracing::error!("Could not find timestamp 'created' for Watchlist-Item after Batch-Get. This is a bug. Skipping Item.");
                                None
                            },
                        },
                    )
                    .collect::<Vec<_>>();
                // BatchGetItem responds with any order, so we need to restore the order from the query manually
                items.sort_by(|l, r| {
                    if scan_index_forward {
                        l.created.cmp(&r.created)
                    } else {
                        l.created.cmp(&r.created).reverse()
                    }
                });
                Ok(PaginatedResult {
                    page: Page {
                        from: created_guard.unwrap_or(default_guard),
                        size: items.len() as u64,
                    },
                    items,
                    total: None,
                    next_after: Some(next.created),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {

    mod watch {
        use crate::{
            repository::MockWatchlistItemDynamoDbRepository,
            service::{ItemWatchListService, ItemWatchListServiceImpl, WatchItemError},
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
            operation::put_item::PutItemOutput,
        };
        use common::{shop_id::ShopId, shops_item_id::ShopsItemId};
        use fake::{Fake, Faker};
        use item_dynamodb::repository::MockItemDynamoDbRepository;
        use item_service::get_service::GetItemServiceImpl;

        #[tokio::test]
        async fn should_watch_when_success() {
            let mut item_repository = MockItemDynamoDbRepository::default();
            item_repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));

            let mut watchlist_repository = MockWatchlistItemDynamoDbRepository::default();
            watchlist_repository
                .expect_put_watchlist_record()
                .return_once(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));
            let get_item_service = GetItemServiceImpl::new(&item_repository);

            let service = ItemWatchListServiceImpl::new(
                &watchlist_repository,
                &item_repository,
                &get_item_service,
            );
            service
                .watch(&Faker.fake(), &Faker.fake(), &Faker.fake())
                .await
                .unwrap();
        }

        #[tokio::test]
        async fn should_err_item_not_found_when_item_not_exists() {
            let mut item_repository = MockItemDynamoDbRepository::default();
            item_repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));

            let watchlist_repository = MockWatchlistItemDynamoDbRepository::default();
            let get_item_service = GetItemServiceImpl::new(&item_repository);

            let service = ItemWatchListServiceImpl::new(
                &watchlist_repository,
                &item_repository,
                &get_item_service,
            );
            let shop_id = ShopId::new();
            let shops_item_id = ShopsItemId::new();
            let actual = service
                .watch(&Faker.fake(), &shop_id, &shops_item_id)
                .await
                .unwrap_err();

            match actual {
                WatchItemError::ItemNotFound(err_shop_id, err_shops_item_id) => {
                    assert_eq!(shop_id, err_shop_id);
                    assert_eq!(shops_item_id, err_shops_item_id);
                }
                err => panic!("Expected 'WatchItemError::ItemNotFound' but got '{err}'"),
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
        async fn should_propagate_sdk_error_get_item(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut item_repository = MockItemDynamoDbRepository::default();
            item_repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));

            let watchlist_repository = MockWatchlistItemDynamoDbRepository::default();
            let get_item_service = GetItemServiceImpl::new(&item_repository);

            let service = ItemWatchListServiceImpl::new(
                &watchlist_repository,
                &item_repository,
                &get_item_service,
            );

            let actual = service
                .watch(&Faker.fake(), &Faker.fake(), &Faker.fake())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                WatchItemError::SdkGetItemError(_) => {}
                err => panic!("Expected 'WatchItemError::SdkGetItemError', got '{err}'"),
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
        async fn should_propagate_sdk_error_put_item(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::put_item::PutItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut item_repository = MockItemDynamoDbRepository::default();
            item_repository
                .expect_get_item_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));

            let mut watchlist_repository = MockWatchlistItemDynamoDbRepository::default();
            watchlist_repository
                .expect_put_watchlist_record()
                .return_once(|_| Box::pin(async { Err(expected) }));
            let get_item_service = GetItemServiceImpl::new(&item_repository);

            let service = ItemWatchListServiceImpl::new(
                &watchlist_repository,
                &item_repository,
                &get_item_service,
            );

            let actual = service
                .watch(&Faker.fake(), &Faker.fake(), &Faker.fake())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                WatchItemError::SdkPutItemError(_) => {}
                err => panic!("Expected 'WatchItemError::SdkPutItemError', got '{err}'"),
            }
        }
    }

    mod unwatch {
        use crate::{
            repository::MockWatchlistItemDynamoDbRepository,
            service::{ItemWatchListService, ItemWatchListServiceImpl, WatchItemError},
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
            operation::delete_item::DeleteItemOutput,
        };
        use common::user_id::UserId;
        use fake::{Fake, Faker};
        use item_dynamodb::repository::MockItemDynamoDbRepository;
        use item_service::get_service::GetItemServiceImpl;
        use time::OffsetDateTime;

        #[tokio::test]
        async fn should_unwatch_when_success() {
            let item_repository = MockItemDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistItemDynamoDbRepository::default();
            watchlist_repository
                .expect_get_watchlist_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            watchlist_repository
                .expect_delete_watchlist_record()
                .return_once(|_, _| Box::pin(async { Ok(DeleteItemOutput::builder().build()) }));
            let get_item_service = GetItemServiceImpl::new(&item_repository);

            let service = ItemWatchListServiceImpl::new(
                &watchlist_repository,
                &item_repository,
                &get_item_service,
            );
            service.unwatch(&Faker.fake(), &Faker.fake()).await.unwrap();
        }

        #[tokio::test]
        async fn should_err_watchlist_timestamp_not_found_when_no_watched_item_with_timestamp_exists()
         {
            let item_repository = MockItemDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistItemDynamoDbRepository::default();
            watchlist_repository
                .expect_get_watchlist_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));
            let get_item_service = GetItemServiceImpl::new(&item_repository);

            let service = ItemWatchListServiceImpl::new(
                &watchlist_repository,
                &item_repository,
                &get_item_service,
            );
            let user_id = UserId::new();
            let timestamp = OffsetDateTime::now_utc();
            let actual = service.unwatch(&user_id, &timestamp).await.unwrap_err();

            match actual {
                WatchItemError::WatchlistTimestampNotFound(err_user_id, err_timestamp) => {
                    assert_eq!(user_id, err_user_id);
                    assert_eq!(timestamp, err_timestamp);
                }
                err => {
                    panic!("Expected 'WatchItemError::WatchlistTimestampNotFound' but got '{err}'")
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
        async fn should_propagate_sdk_error_get_item(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let item_repository = MockItemDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistItemDynamoDbRepository::default();
            watchlist_repository
                .expect_get_watchlist_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let get_item_service = GetItemServiceImpl::new(&item_repository);

            let service = ItemWatchListServiceImpl::new(
                &watchlist_repository,
                &item_repository,
                &get_item_service,
            );

            let actual = service.unwatch(&Faker.fake(), &Faker.fake()).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                WatchItemError::SdkGetItemError(_) => {}
                err => panic!("Expected 'WatchItemError::SdkGetItemError', got '{err}'"),
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
        async fn should_propagate_sdk_error_delete_item(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::delete_item::DeleteItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let item_repository = MockItemDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistItemDynamoDbRepository::default();
            watchlist_repository
                .expect_get_watchlist_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            watchlist_repository
                .expect_delete_watchlist_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let get_item_service = GetItemServiceImpl::new(&item_repository);

            let service = ItemWatchListServiceImpl::new(
                &watchlist_repository,
                &item_repository,
                &get_item_service,
            );

            let actual = service.unwatch(&Faker.fake(), &Faker.fake()).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                WatchItemError::SdkDeleteItemError(_) => {}
                err => panic!("Expected 'WatchItemError::SdkDeleteItemError', got '{err}'"),
            }
        }
    }

    mod view_watchlist {
        use crate::{
            repository::MockWatchlistItemDynamoDbRepository,
            service::{ItemWatchListService, ItemWatchListServiceImpl, WatchItemError},
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::{currency::domain::Currency, language::domain::Language};
        use fake::{Fake, Faker};
        use item_dynamodb::repository::MockItemDynamoDbRepository;
        use item_service::get_service::MockGetItemService;

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
        async fn should_propagate_sdk_error_query(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::query::QueryError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let item_repository = MockItemDynamoDbRepository::default();
            let mut watchlist_repository = MockWatchlistItemDynamoDbRepository::default();
            watchlist_repository
                .expect_query_watchlist_records()
                .return_once(|_, _, _, _| Box::pin(async { Err(expected) }));
            let get_item_service = MockGetItemService::default();
            let service = ItemWatchListServiceImpl::new(
                &watchlist_repository,
                &item_repository,
                &get_item_service,
            );
            let actual = service
                .view_watchlist(&Faker.fake(), &[Language::De], &Currency::Eur, &None, &None)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                WatchItemError::SdkQueryError(_) => {}
                err => panic!("Expected 'WatchItemError::SdkQueryError', got '{err}'"),
            }
        }
    }
}
