use crate::{
    dynamodb::record::{WatchlistProductRecord, mk_gsi1_pk, mk_lsi1_sk, mk_pk, mk_sk},
    dynamodb::record_update::WatchlistProductRecordUpdate,
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
    dynamodb_update::DynamoDbUpdate, pagination::cursor::Cursor, product_id::ProductId,
    shop_id::ShopId, shops_product_id::ShopsProductId, user_id::UserId,
};
use time::{OffsetDateTime, macros::datetime};
use tracing::error;

#[async_trait::async_trait]
#[mockall::automock]
pub trait WatchlistProductDynamoDbRepository {
    async fn query_watchlist_records_all(
        &self,
        user_id: &UserId,
        scan_index_forward: bool,
    ) -> Result<Vec<WatchlistProductRecord>, SdkError<QueryError>>;

    async fn query_watchlist_records(
        &self,
        user_id: &UserId,
        cursor: &Cursor<OffsetDateTime>,
        scan_index_forward: bool,
    ) -> Result<Vec<WatchlistProductRecord>, SdkError<QueryError>>;

    async fn count_watchlist_records(
        &self,
        user_id: &UserId,
        cursor: &Cursor<OffsetDateTime>,
        scan_index_forward: bool,
    ) -> Result<u64, SdkError<QueryError>>;

    async fn put_watchlist_record(
        &self,
        record: WatchlistProductRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError>>;

    async fn get_watchlist_record(
        &self,
        user_id: &UserId,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<Option<WatchlistProductRecord>, SdkError<GetItemError>>;

    async fn delete_watchlist_record(
        &self,
        user_id: &UserId,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<DeleteItemOutput, SdkError<DeleteItemError>>;

    async fn update_watchlist_record(
        &self,
        user_id: &UserId,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
        update: WatchlistProductRecordUpdate,
    ) -> Result<Option<WatchlistProductRecord>, SdkError<UpdateItemError>>;

    async fn query_user_ids_watching_product(
        &self,
        product_id: &ProductId,
    ) -> Result<Vec<(UserId, bool)>, SdkError<QueryError>>;
}

#[derive(Debug, Clone)]
pub struct WatchlistProductDynamoDbRepositoryImpl<'a> {
    client: &'a Client,
    table: String,
}

impl<'a> WatchlistProductDynamoDbRepositoryImpl<'a> {
    pub fn new(client: &'a Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }
}

/// Table sort-key prefix shared by all watchlist records (`product#watch#shop_id#…`).
/// Used in a `begins_with` filter expression to enforce double-sided bounds: only items
/// whose `sk` starts with this prefix — i.e. genuine watchlist records — are returned,
/// preventing other record types in the same partition (e.g. notification records whose
/// `lsi1_sk` could otherwise fall inside the queried range) from leaking into results.
const SK_PREFIX: &str = "product#watch#";

#[async_trait::async_trait]
impl<'a> WatchlistProductDynamoDbRepository for WatchlistProductDynamoDbRepositoryImpl<'a> {
    async fn query_watchlist_records_all(
        &self,
        user_id: &UserId,
        scan_index_forward: bool,
    ) -> Result<Vec<WatchlistProductRecord>, SdkError<QueryError>> {
        let watchlist_records = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("#pk = :pk_val AND begins_with(#sk, :sk_prefix)")
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#sk", "sk")
            .expression_attribute_values(
                ":pk_val",
                AttributeValue::S(mk_pk(user_id)),
            )
            .expression_attribute_values(
                ":sk_prefix",
                AttributeValue::S("product#watch#".to_string()),
            )
            .scan_index_forward(scan_index_forward)
            .into_paginator()
            .send()
            .try_collect()
            .await?
            .into_iter()
            .flat_map(|qo| qo.items.unwrap_or_default())
            .map(serde_dynamo::from_item::<_, WatchlistProductRecord>)
            .filter_map(|res| match res {
                Ok(record) => Some(record),
                Err(err) => {
                    error!(userId = %user_id, error = %err, type = %std::any::type_name::<WatchlistProductRecord>(), "Failed deserializing.");
                    None
                }
            })
            .collect();

        Ok(watchlist_records)
    }

    async fn query_watchlist_records(
        &self,
        user_id: &UserId,
        cursor: &Cursor<OffsetDateTime>,
        scan_index_forward: bool,
    ) -> Result<Vec<WatchlistProductRecord>, SdkError<QueryError>> {
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
            .filter_expression("begins_with(#sk, :sk_prefix)")
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#lsi1_sk", "lsi1_sk")
            .expression_attribute_names("#sk", "sk")
            .expression_attribute_values(":pk_val", AttributeValue::S(mk_pk(user_id)))
            .expression_attribute_values(
                ":lsi1_sk_val_exclusive_guard",
                AttributeValue::S(
                    mk_lsi1_sk(&exclusive_guard).map_err(SdkError::construction_failure)?,
                ),
            )
            .expression_attribute_values(
                ":sk_prefix",
                AttributeValue::S(SK_PREFIX.to_string()),
            )
            .limit(cursor.size as i32)
            .scan_index_forward(scan_index_forward)
            .send()
            .await?
            .items
            .unwrap_or_default()
            .into_iter()
            .map(serde_dynamo::from_item::<_, WatchlistProductRecord>)
            .filter_map(|res| match res {
                Ok(record) => Some(record),
                Err(err) => {
                    error!(userId = %user_id, error = %err, type = %std::any::type_name::<WatchlistProductRecord>(), "Failed deserializing.");
                    None
                }
            })
            .collect();

        Ok(records)
    }

    async fn count_watchlist_records(
        &self,
        user_id: &UserId,
        cursor: &Cursor<OffsetDateTime>,
        scan_index_forward: bool,
    ) -> Result<u64, SdkError<QueryError>> {
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

        let count = self
            .client
            .query()
            .table_name(&self.table)
            .index_name("lsi1")
            .key_condition_expression(key_condition_expression)
            .filter_expression("begins_with(#sk, :sk_prefix)")
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#lsi1_sk", "lsi1_sk")
            .expression_attribute_names("#sk", "sk")
            .expression_attribute_values(":pk_val", AttributeValue::S(mk_pk(user_id)))
            .expression_attribute_values(
                ":lsi1_sk_val_exclusive_guard",
                AttributeValue::S(
                    mk_lsi1_sk(&exclusive_guard).map_err(SdkError::construction_failure)?,
                ),
            )
            .expression_attribute_values(":sk_prefix", AttributeValue::S(SK_PREFIX.to_string()))
            .scan_index_forward(scan_index_forward)
            .select(aws_sdk_dynamodb::types::Select::Count)
            .send()
            .await?
            .count;

        Ok(count as u64)
    }

    async fn put_watchlist_record(
        &self,
        record: WatchlistProductRecord,
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
        shops_product_id: &ShopsProductId,
    ) -> Result<Option<WatchlistProductRecord>, SdkError<GetItemError>> {
        let record = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk(shop_id, shops_product_id)))
            .send()
            .await?
            .item
            .map(serde_dynamo::from_item::<_, WatchlistProductRecord>)
            .and_then(|res| match res {
                Ok(record) => Some(record),
                Err(err) => {
                    error!(
                        userId = %user_id,
                        shopId = %shop_id,
                        shopsProductId = %shops_product_id,
                        error = %err,
                        type = %std::any::type_name::<WatchlistProductRecord>(),
                        "Failed deserializing WatchlistProductRecord."
                    );
                    None
                }
            });

        Ok(record)
    }

    async fn delete_watchlist_record(
        &self,
        user_id: &UserId,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<DeleteItemOutput, SdkError<DeleteItemError>> {
        self.client
            .delete_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk(shop_id, shops_product_id)))
            .send()
            .await
    }

    async fn update_watchlist_record(
        &self,
        user_id: &UserId,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
        update: WatchlistProductRecordUpdate,
    ) -> Result<Option<WatchlistProductRecord>, SdkError<UpdateItemError>> {
        let update_expr = update.into_update_expr()?;

        self.client
            .update_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk(shop_id, shops_product_id)))
            .update_expression(update_expr.update_expr)
            .set_expression_attribute_names(Some(update_expr.expr_attr_names))
            .set_expression_attribute_values(Some(update_expr.expr_attr_values))
            .return_values(ReturnValue::AllNew)
            .send()
            .await
            .map(|output| output.attributes)
            .map(|attr_opt| {
                attr_opt
                    .map(serde_dynamo::from_item)
                    .and_then(|record_res| match record_res {
                        Ok(search_filter_record) => Some(search_filter_record),
                        Err(err) => {
                            error!(
                                userId = %user_id,
                                shopId = %shop_id,
                                shopsProductId = %shops_product_id,
                                error = %err,
                                type = %std::any::type_name::<WatchlistProductRecord>(),
                                "Failed deserializing WatchlistProductRecord."
                            );
                            None
                        }
                    })
            })
    }

    async fn query_user_ids_watching_product(
        &self,
        product_id: &ProductId,
    ) -> Result<Vec<(UserId, bool)>, SdkError<QueryError>> {
        let user_records = self
            .client
            .query()
            .table_name(&self.table)
            .index_name("gsi1")
            .key_condition_expression("#gsi1_pk = :gsi1_pk_val AND begins_with(#gsi1_sk, :gsi1_sk_val)")
            .expression_attribute_names("#gsi1_pk", "gsi1_pk")
            .expression_attribute_names("#gsi1_sk", "gsi1_sk")
            .expression_attribute_values(
                ":gsi1_pk_val",
                AttributeValue::S(mk_gsi1_pk(product_id)),
            )
            .expression_attribute_values(
                ":gsi1_sk_val",
                AttributeValue::S("watch#user#".to_owned()),
            )
            .into_paginator()
            .send()
            .try_collect()
            .await?
            .into_iter()
            .flat_map(|qo| qo.items.unwrap_or_default())
            .map(serde_dynamo::from_item::<_, WatchlistProductRecord>)
            .filter_map(|res| match res {
                Ok(record) => Some((record.user_id, record.notifications)),
                Err(err) => {
                    error!(productId = %product_id, error = %err, type = %std::any::type_name::<WatchlistProductRecord>(), "Failed deserializing.");
                    None
                }
            })
            .collect();

        Ok(user_records)
    }
}
