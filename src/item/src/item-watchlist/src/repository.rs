use crate::{
    record::{WatchlistItemRecord, mk_gsi1_pk, mk_lsi1_sk, mk_pk, mk_sk},
    record_update::WatchlistItemRecordUpdate,
};
use aws_sdk_dynamodb::{
    Client,
    error::SdkError,
    operation::{
        delete_item::{DeleteItemError, DeleteItemOutput},
        get_item::GetItemError,
        put_item::{PutItemError, PutItemOutput},
        query::QueryError,
        update_item::UpdateItemError,
    },
    types::{AttributeValue, ReturnValue},
};
use common::{
    dynamodb_update::DynamoDbUpdate, item_id::ItemId, pagination::cursor::Cursor, shop_id::ShopId,
    shops_item_id::ShopsItemId, user_id::UserId,
};
use time::{OffsetDateTime, macros::datetime};
use tracing::error;
use user_dynamodb::user_record::UserRecord;

#[async_trait::async_trait]
#[mockall::automock]
pub trait WatchlistItemDynamoDbRepository {
    async fn query_watchlist_records(
        &self,
        user_id: &UserId,
        cursor: &Cursor<OffsetDateTime>,
        scan_index_forward: bool,
    ) -> Result<Vec<WatchlistItemRecord>, SdkError<QueryError>>;

    async fn put_watchlist_record(
        &self,
        record: WatchlistItemRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError>>;

    async fn get_watchlist_record(
        &self,
        user_id: &UserId,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
    ) -> Result<Option<WatchlistItemRecord>, SdkError<GetItemError>>;

    async fn delete_watchlist_record(
        &self,
        user_id: &UserId,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
    ) -> Result<DeleteItemOutput, SdkError<DeleteItemError>>;

    async fn update_watchlist_record(
        &self,
        user_id: &UserId,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
        update: WatchlistItemRecordUpdate,
    ) -> Result<Option<WatchlistItemRecord>, SdkError<UpdateItemError>>;

    async fn query_user_records_with_notifications(
        &self,
        item_id: &ItemId,
    ) -> Result<Vec<UserRecord>, SdkError<QueryError>>;
}

#[derive(Debug, Clone)]
pub struct WatchlistItemDynamoDbRepositoryImpl<'a> {
    client: &'a Client,
    table: String,
}

impl<'a> WatchlistItemDynamoDbRepositoryImpl<'a> {
    pub fn new(client: &'a Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }
}

#[async_trait::async_trait]
impl<'a> WatchlistItemDynamoDbRepository for WatchlistItemDynamoDbRepositoryImpl<'a> {
    async fn query_watchlist_records(
        &self,
        user_id: &UserId,
        cursor: &Cursor<OffsetDateTime>,
        scan_index_forward: bool,
    ) -> Result<Vec<WatchlistItemRecord>, SdkError<QueryError>> {
        let exclusive_guard = if scan_index_forward {
            cursor
                .search_after
                .unwrap_or(datetime!(2000 - 01 - 01 0:00 UTC))
        } else {
            cursor.search_after.unwrap_or(OffsetDateTime::now_utc())
        };
        let key_condition_expression = if scan_index_forward {
            "#pk = :pk_val AND #lsi1_sk > :lsi1_sk_val_exclusive_guard"
        } else {
            "#pk = :pk_val AND #lsi1_sk < :lsi1_sk_val_exclusive_guard"
        };

        let records = self
            .client
            .query()
            .table_name(&self.table)
            .index_name("lsi1")
            .key_condition_expression(key_condition_expression)
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#lsi1_sk", "lsi1_sk")
            .expression_attribute_values(
                ":pk_val",
                AttributeValue::S(mk_pk(user_id)),
            )
            .expression_attribute_values(
                ":lsi1_sk_val_exclusive_guard",
                AttributeValue::S(mk_lsi1_sk(&exclusive_guard).map_err(SdkError::construction_failure)?),
            )
            .limit(cursor.size as i32)
            .scan_index_forward(scan_index_forward)
            .send()
            .await?
            .items
            .unwrap_or_default()
            .into_iter()
            .map(serde_dynamo::from_item::<_, WatchlistItemRecord>)
            .filter_map(|res| match res {
                Ok(record) => Some(record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<WatchlistItemRecord>(), "Failed deserializing.");
                    None
                }
            })
            .collect();

        Ok(records)
    }

    async fn put_watchlist_record(
        &self,
        record: WatchlistItemRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError>> {
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(
                serde_dynamo::to_item(record).map_err(SdkError::construction_failure)?,
            ))
            .send()
            .await
    }

    async fn get_watchlist_record(
        &self,
        user_id: &UserId,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
    ) -> Result<Option<WatchlistItemRecord>, SdkError<GetItemError>> {
        let record = self.client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk(shop_id, shops_item_id)))
            .send()
            .await?
            .item
            .map(serde_dynamo::from_item::<_, WatchlistItemRecord>)
            .and_then(|res| match res {
                Ok(record) => Some(record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<WatchlistItemRecord>(), "Failed deserializing.");
                    None
                }
            });

        Ok(record)
    }

    async fn delete_watchlist_record(
        &self,
        user_id: &UserId,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
    ) -> Result<DeleteItemOutput, SdkError<DeleteItemError>> {
        self.client
            .delete_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk(shop_id, shops_item_id)))
            .send()
            .await
    }

    async fn update_watchlist_record(
        &self,
        user_id: &UserId,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
        update: WatchlistItemRecordUpdate,
    ) -> Result<Option<WatchlistItemRecord>, SdkError<UpdateItemError>> {
        let update_expr = update.into_update_expr()?;

        self.client
            .update_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk(shop_id, shops_item_id)))
            .update_expression(update_expr.update_expr)
            .set_expression_attribute_names(Some(update_expr.expr_attr_names))
            .set_expression_attribute_values(Some(update_expr.expr_attr_values))
            .return_values(ReturnValue::AllNew)
            .send()
            .await
            .map(|output| output.attributes)
            .map(|attr_opt| attr_opt.map(serde_dynamo::from_item).and_then(|record_res| match record_res {
                Ok(search_filter_record) => Some(search_filter_record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<WatchlistItemRecord>(), "Failed deserializing WatchlistItemRecord.");
                    None
                }
            }))
    }

    // this is why we heavily denormalize and store the entire user-record with every watchlist-entry
    async fn query_user_records_with_notifications(
        &self,
        item_id: &ItemId,
    ) -> Result<Vec<UserRecord>, SdkError<QueryError>> {
        let user_records = self
            .client
            .query()
            .table_name(&self.table)
            .index_name("gsi1")
            .key_condition_expression("#pk = :pk_val AND ")
            .key_condition_expression("#gsi1_pk = :gsi1_pk_val AND begins_with(#gsi1_sk, :gsi1_sk_val)")
            .expression_attribute_names("#gsi1_pk", "gsi1_pk")
            .expression_attribute_names("#gsi1_sk", "gsi1_sk")
            .expression_attribute_values(
                ":gsi1_pk_val",
                AttributeValue::S(mk_gsi1_pk(item_id)),
            )
            .expression_attribute_values(
                ":gsi1_sk_val",
                AttributeValue::S("user#".to_owned()),
            )
            .into_paginator()
            .send()
            .try_collect()
            .await?
            .into_iter()
            .flat_map(|qo| qo.items.unwrap_or_default())
            .map(serde_dynamo::from_item::<_, WatchlistItemRecord>)
            .filter_map(|res| match res {
                Ok(record) => Some(record.user_record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<WatchlistItemRecord>(), "Failed deserializing.");
                    None
                }
            })
            .collect();

        Ok(user_records)
    }
}
