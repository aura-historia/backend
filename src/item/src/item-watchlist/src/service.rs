use crate::{
    record::{WatchlistItemRecord, mk_pk, mk_sk},
    repository::WatchlistItemDynamoDbRepository,
};
use aws_sdk_dynamodb::{
    config::http::HttpResponse, error::SdkError, operation::put_item::PutItemError,
};
use common::{shop_id::ShopId, shops_item_id::ShopsItemId, user_id::UserId};
use item_dynamodb::repository::ItemDynamoDbRepository;
use time::OffsetDateTime;

#[derive(thiserror::Error, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum WatchItemError {
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

#[cfg(feature = "api")]
pub mod api {
    use crate::service::WatchItemError;
    use common::api::error::ApiError;
    use common::api::error_code::{ITEM_NOT_FOUND, WATCHLIST_ENTRY_NOT_FOUND};
    use tracing::error;

    impl From<WatchItemError> for ApiError {
        fn from(err: WatchItemError) -> Self {
            match err {
                WatchItemError::ItemNotFound(_, _) => ApiError::not_found(ITEM_NOT_FOUND),
                WatchItemError::WatchlistTimestampNotFound(_, _) => {
                    ApiError::not_found(WATCHLIST_ENTRY_NOT_FOUND)
                }
                WatchItemError::SdkGetItemError(err) => {
                    error!(error = ?err, "Encountered SdkGetItemError while getting item.");
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
}

pub struct ItemWatchListServiceImpl<'a> {
    watchlist_repository: &'a (dyn WatchlistItemDynamoDbRepository + Sync),
    item_repository: &'a (dyn ItemDynamoDbRepository + Sync),
}

impl<'a> ItemWatchListServiceImpl<'a> {
    pub fn new(
        watchlist_repository: &'a (dyn WatchlistItemDynamoDbRepository + Sync),
        item_repository: &'a (dyn ItemDynamoDbRepository + Sync),
    ) -> Self {
        Self {
            watchlist_repository,
            item_repository,
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
}
