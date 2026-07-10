use crate::{
    dynamodb::record::{WatchlistProductRecord, mk_gsi1_pk, mk_lsi1_sk, mk_pk, mk_sk},
    dynamodb::record_update::WatchlistProductRecordUpdate,
};
use aws_sdk_dynamodb::{
    Client,
    error::SdkError,
    operation::{
        batch_write_item::{BatchWriteItemError, BatchWriteItemOutput},
        delete_item::{DeleteItemError, DeleteItemOutput},
        get_item::GetItemError,
        put_item::{PutItemError, PutItemOutput},
        query::QueryError,
        update_item::UpdateItemError,
    },
    types::{AttributeValue, DeleteRequest, ReturnValue, WriteRequest},
};
use common::{
    batch::Batch, dynamodb_update::DynamoDbUpdate, pagination::cursor::Cursor,
    product_id::ProductId, shop_id::ShopId, shops_product_id::ShopsProductId, user_id::UserId,
};
use std::collections::HashMap;
use time::{Duration, OffsetDateTime};
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
    ) -> Result<Vec<WatchlistProductRecord>, SdkError<QueryError>>;

    async fn delete_watchlist_records(
        &self,
        keys: Batch<(UserId, ShopId, ShopsProductId), 25>,
    ) -> Result<BatchWriteItemOutput, SdkError<BatchWriteItemError>>;
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

/// Lower bound for the `lsi1_sk` of all watchlist records.
const LSI1_SK_LOWER_BOUND: &str = "product#watch#created#";
/// Upper bound for the `lsi1_sk` of all watchlist records.
const LSI1_SK_UPPER_BOUND: &str = "product#watch#created#\u{ffff}";

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
        let (lsi1_sk_lower, lsi1_sk_upper) = if scan_index_forward {
            let lower = match cursor.search_after {
                Some(created) => mk_lsi1_sk(&(created + Duration::NANOSECOND)),
                None => LSI1_SK_LOWER_BOUND.to_string(),
            };
            (lower, LSI1_SK_UPPER_BOUND.to_string())
        } else {
            let upper = match cursor.search_after {
                Some(created) => mk_lsi1_sk(&(created - Duration::NANOSECOND)),
                None => LSI1_SK_UPPER_BOUND.to_string(),
            };
            (LSI1_SK_LOWER_BOUND.to_string(), upper)
        };

        let records = self
            .client
            .query()
            .table_name(&self.table)
            .index_name("lsi1")
            .key_condition_expression(
                "#pk = :pk_val AND #lsi1_sk BETWEEN :lsi1_sk_lower AND :lsi1_sk_upper",
            )
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#lsi1_sk", "lsi1_sk")
            .expression_attribute_values(":pk_val", AttributeValue::S(mk_pk(user_id)))
            .expression_attribute_values(
                ":lsi1_sk_lower",
                AttributeValue::S(lsi1_sk_lower),
            )
            .expression_attribute_values(
                ":lsi1_sk_upper",
                AttributeValue::S(lsi1_sk_upper),
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
        let (lsi1_sk_lower, lsi1_sk_upper) = if scan_index_forward {
            let lower = match cursor.search_after {
                Some(created) => mk_lsi1_sk(&(created + Duration::NANOSECOND)),
                None => LSI1_SK_LOWER_BOUND.to_string(),
            };
            (lower, LSI1_SK_UPPER_BOUND.to_string())
        } else {
            let upper = match cursor.search_after {
                Some(created) => mk_lsi1_sk(&(created - Duration::NANOSECOND)),
                None => LSI1_SK_UPPER_BOUND.to_string(),
            };
            (LSI1_SK_LOWER_BOUND.to_string(), upper)
        };

        let count = self
            .client
            .query()
            .table_name(&self.table)
            .index_name("lsi1")
            .key_condition_expression(
                "#pk = :pk_val AND #lsi1_sk BETWEEN :lsi1_sk_lower AND :lsi1_sk_upper",
            )
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#lsi1_sk", "lsi1_sk")
            .expression_attribute_values(":pk_val", AttributeValue::S(mk_pk(user_id)))
            .expression_attribute_values(":lsi1_sk_lower", AttributeValue::S(lsi1_sk_lower))
            .expression_attribute_values(":lsi1_sk_upper", AttributeValue::S(lsi1_sk_upper))
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
    ) -> Result<Vec<WatchlistProductRecord>, SdkError<QueryError>> {
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
                Ok(record) => Some(record),
                Err(err) => {
                    error!(productId = %product_id, error = %err, type = %std::any::type_name::<WatchlistProductRecord>(), "Failed deserializing.");
                    None
                }
            })
            .collect();

        Ok(user_records)
    }

    async fn delete_watchlist_records(
        &self,
        keys: Batch<(UserId, ShopId, ShopsProductId), 25>,
    ) -> Result<BatchWriteItemOutput, SdkError<BatchWriteItemError>> {
        let mut requests = Vec::with_capacity(keys.len());
        for (user_id, shop_id, shops_product_id) in keys {
            let delete = DeleteRequest::builder()
                .key("pk", AttributeValue::S(mk_pk(&user_id)))
                .key("sk", AttributeValue::S(mk_sk(&shop_id, &shops_product_id)))
                .build()
                .map_err(SdkError::construction_failure)?;
            requests.push(WriteRequest::builder().delete_request(delete).build());
        }
        self.client
            .batch_write_item()
            .set_request_items(Some(HashMap::from([(self.table.clone(), requests)])))
            .send()
            .await
    }
}
