use crate::{
    dynamodb::notification_record::{NotificationRecord, mk_lsi2_sk_product_prefix, mk_pk, mk_sk},
    dynamodb::notification_record_update::NotificationRecordUpdate,
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
    batch::Batch, dynamodb_update::DynamoDbUpdate, event_id::EventId, pagination::cursor::Cursor,
    product_id::ProductId, user_id::UserId,
};
use std::collections::HashMap;
use tracing::error;

#[async_trait::async_trait]
#[mockall::automock]
pub trait NotificationDynamoDbRepository {
    async fn query_notification_records(
        &self,
        user_id: &UserId,
        cursor: &Cursor<EventId>,
        scan_index_forward: bool,
    ) -> Result<Vec<NotificationRecord>, SdkError<QueryError>>;

    async fn count_notification_records(
        &self,
        user_id: &UserId,
        cursor: &Cursor<EventId>,
        scan_index_forward: bool,
    ) -> Result<u64, SdkError<QueryError>>;

    async fn put_notification_record(
        &self,
        record: NotificationRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError>>;

    async fn put_notification_records(
        &self,
        records: Batch<NotificationRecord, 25>,
    ) -> Result<BatchWriteItemOutput, SdkError<BatchWriteItemError>>;

    async fn get_notification_record(
        &self,
        user_id: &UserId,
        origin_event_id: &EventId,
    ) -> Result<Option<NotificationRecord>, SdkError<GetItemError>>;

    async fn update_notification_record(
        &self,
        user_id: &UserId,
        origin_event_id: &EventId,
        update: NotificationRecordUpdate,
    ) -> Result<Option<NotificationRecord>, SdkError<UpdateItemError>>;

    async fn query_all_notification_records(
        &self,
        user_id: &UserId,
    ) -> Result<Vec<NotificationRecord>, SdkError<QueryError>>;

    async fn delete_notification_record(
        &self,
        user_id: &UserId,
        origin_event_id: &EventId,
    ) -> Result<DeleteItemOutput, SdkError<DeleteItemError>>;

    async fn delete_notification_records(
        &self,
        user_id: &UserId,
        origin_event_ids: &Batch<EventId, 25>,
    ) -> Result<BatchWriteItemOutput, SdkError<BatchWriteItemError>>;

    async fn query_product_notification_records(
        &self,
        user_id: &UserId,
        product_id: &ProductId,
        limit: Option<i32>,
        scan_index_forward: bool,
    ) -> Result<Vec<NotificationRecord>, SdkError<QueryError>>;
}

#[derive(Debug, Clone)]
pub struct NotificationDynamoDbRepositoryImpl<'a> {
    client: &'a Client,
    table: String,
}

impl<'a> NotificationDynamoDbRepositoryImpl<'a> {
    pub fn new(client: &'a Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }
}

const SK_LOWER_BOUND: &str = "user#notification#origin_event_id#";
const SK_UPPER_BOUND: &str = "user#notification#origin_event_id#\u{ffff}";

#[async_trait::async_trait]
impl<'a> NotificationDynamoDbRepository for NotificationDynamoDbRepositoryImpl<'a> {
    async fn query_notification_records(
        &self,
        user_id: &UserId,
        cursor: &Cursor<EventId>,
        scan_index_forward: bool,
    ) -> Result<Vec<NotificationRecord>, SdkError<QueryError>> {
        let exclusive_guard = if scan_index_forward {
            cursor
                .search_after
                .map(|id| mk_sk(&id))
                .unwrap_or_else(|| SK_LOWER_BOUND.to_string())
        } else {
            cursor
                .search_after
                .map(|id| mk_sk(&id))
                .unwrap_or_else(|| SK_UPPER_BOUND.to_string())
        };
        let key_condition_expression = if scan_index_forward {
            "#pk = :pk_val AND #sk > :sk_val_exclusive_guard"
        } else {
            "#pk = :pk_val AND #sk < :sk_val_exclusive_guard"
        };

        let records = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression(key_condition_expression)
            .filter_expression("#sk >= :sk_lower")
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#sk", "sk")
            .expression_attribute_values(":pk_val", AttributeValue::S(mk_pk(user_id)))
            .expression_attribute_values(
                ":sk_val_exclusive_guard",
                AttributeValue::S(exclusive_guard),
            )
            .expression_attribute_values(":sk_lower", AttributeValue::S(SK_LOWER_BOUND.to_string()))
            .limit(cursor.size as i32)
            .scan_index_forward(scan_index_forward)
            .send()
            .await?
            .items
            .unwrap_or_default()
            .into_iter()
            .map(serde_dynamo::from_item::<_, NotificationRecord>)
            .filter_map(|res| match res {
                Ok(record) => Some(record),
                Err(err) => {
                    error!(
                        userId = %user_id,
                        error = %err,
                        r#type = %std::any::type_name::<NotificationRecord>(),
                        "Failed deserializing."
                    );
                    None
                }
            })
            .collect();

        Ok(records)
    }

    async fn count_notification_records(
        &self,
        user_id: &UserId,
        cursor: &Cursor<EventId>,
        scan_index_forward: bool,
    ) -> Result<u64, SdkError<QueryError>> {
        let exclusive_guard = if scan_index_forward {
            cursor
                .search_after
                .map(|id| mk_sk(&id))
                .unwrap_or_else(|| SK_LOWER_BOUND.to_string())
        } else {
            cursor
                .search_after
                .map(|id| mk_sk(&id))
                .unwrap_or_else(|| SK_UPPER_BOUND.to_string())
        };
        let key_condition_expression = if scan_index_forward {
            "#pk = :pk_val AND #sk > :sk_val_exclusive_guard"
        } else {
            "#pk = :pk_val AND #sk < :sk_val_exclusive_guard"
        };

        let count = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression(key_condition_expression)
            .filter_expression("#sk >= :sk_lower")
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#sk", "sk")
            .expression_attribute_values(":pk_val", AttributeValue::S(mk_pk(user_id)))
            .expression_attribute_values(
                ":sk_val_exclusive_guard",
                AttributeValue::S(exclusive_guard),
            )
            .expression_attribute_values(":sk_lower", AttributeValue::S(SK_LOWER_BOUND.to_string()))
            .scan_index_forward(scan_index_forward)
            .select(aws_sdk_dynamodb::types::Select::Count)
            .send()
            .await?
            .count;

        Ok(count as u64)
    }

    async fn put_notification_record(
        &self,
        record: NotificationRecord,
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

    async fn put_notification_records(
        &self,
        records: Batch<NotificationRecord, 25>,
    ) -> Result<BatchWriteItemOutput, SdkError<BatchWriteItemError>> {
        self.client
            .batch_write_item()
            .set_request_items(Some(HashMap::from([(
                self.table.clone(),
                records.into_dynamodb_write_requests(),
            )])))
            .send()
            .await
    }

    async fn get_notification_record(
        &self,
        user_id: &UserId,
        origin_event_id: &EventId,
    ) -> Result<Option<NotificationRecord>, SdkError<GetItemError>> {
        let record = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk(origin_event_id)))
            .send()
            .await?
            .item
            .map(serde_dynamo::from_item::<_, NotificationRecord>)
            .and_then(|res| match res {
                Ok(record) => Some(record),
                Err(err) => {
                    error!(
                        userId = %user_id,
                        originEventId = %origin_event_id,
                        error = %err,
                        r#type = %std::any::type_name::<NotificationRecord>(),
                        "Failed deserializing NotificationRecord."
                    );
                    None
                }
            });

        Ok(record)
    }

    async fn update_notification_record(
        &self,
        user_id: &UserId,
        origin_event_id: &EventId,
        update: NotificationRecordUpdate,
    ) -> Result<Option<NotificationRecord>, SdkError<UpdateItemError>> {
        let update_expr = update.into_update_expr()?;

        self.client
            .update_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk(origin_event_id)))
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
                        Ok(record) => Some(record),
                        Err(err) => {
                            error!(
                                userId = %user_id,
                                originEventId = %origin_event_id,
                                error = %err,
                                r#type = %std::any::type_name::<NotificationRecord>(),
                                "Failed deserializing NotificationRecord."
                            );
                            None
                        }
                    })
            })
    }

    async fn query_all_notification_records(
        &self,
        user_id: &UserId,
    ) -> Result<Vec<NotificationRecord>, SdkError<QueryError>> {
        let records = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("#pk = :pk_val AND #sk BETWEEN :sk_lower AND :sk_upper")
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#sk", "sk")
            .expression_attribute_values(":pk_val", AttributeValue::S(mk_pk(user_id)))
            .expression_attribute_values(":sk_lower", AttributeValue::S(SK_LOWER_BOUND.to_string()))
            .expression_attribute_values(":sk_upper", AttributeValue::S(SK_UPPER_BOUND.to_string()))
            .into_paginator()
            .send()
            .try_collect()
            .await?
            .into_iter()
            .flat_map(|query_output| query_output.items.unwrap_or_default())
            .filter_map(
                |item| match serde_dynamo::from_item::<_, NotificationRecord>(item) {
                    Ok(record) => Some(record),
                    Err(err) => {
                        error!(
                            userId = %user_id,
                            error = %err,
                            r#type = %std::any::type_name::<NotificationRecord>(),
                            "Failed deserializing."
                        );
                        None
                    }
                },
            )
            .collect();

        Ok(records)
    }

    async fn delete_notification_record(
        &self,
        user_id: &UserId,
        origin_event_id: &EventId,
    ) -> Result<DeleteItemOutput, SdkError<DeleteItemError>> {
        self.client
            .delete_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk(origin_event_id)))
            .send()
            .await
    }

    async fn delete_notification_records(
        &self,
        user_id: &UserId,
        origin_event_ids: &Batch<EventId, 25>,
    ) -> Result<BatchWriteItemOutput, SdkError<BatchWriteItemError>> {
        let write_requests: Vec<WriteRequest> = origin_event_ids
            .iter()
            .map(|id| {
                let mut key = HashMap::new();
                key.insert("pk".to_string(), AttributeValue::S(mk_pk(user_id)));
                key.insert("sk".to_string(), AttributeValue::S(mk_sk(id)));
                WriteRequest::builder()
                    .delete_request(
                        DeleteRequest::builder()
                            .set_key(Some(key))
                            .build()
                            .expect("key is always set"),
                    )
                    .build()
            })
            .collect();

        self.client
            .batch_write_item()
            .set_request_items(Some(HashMap::from([(self.table.clone(), write_requests)])))
            .send()
            .await
    }

    async fn query_product_notification_records(
        &self,
        user_id: &UserId,
        product_id: &ProductId,
        limit: Option<i32>,
        scan_index_forward: bool,
    ) -> Result<Vec<NotificationRecord>, SdkError<QueryError>> {
        let (prefix, _) = mk_lsi2_sk_product_prefix(product_id);

        let mut query_builder = self
            .client
            .query()
            .table_name(&self.table)
            .index_name("lsi2")
            .key_condition_expression("#pk = :pk_val AND begins_with(#lsi2_sk, :prefix)")
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#lsi2_sk", "lsi2_sk")
            .expression_attribute_values(":pk_val", AttributeValue::S(mk_pk(user_id)))
            .expression_attribute_values(":prefix", AttributeValue::S(prefix))
            .scan_index_forward(scan_index_forward);

        if let Some(n) = limit {
            query_builder = query_builder.limit(n);
        }

        let records = query_builder
            .send()
            .await?
            .items
            .unwrap_or_default()
            .into_iter()
            .map(serde_dynamo::from_item::<_, NotificationRecord>)
            .filter_map(|res| match res {
                Ok(record) => Some(record),
                Err(err) => {
                    error!(
                        userId = %user_id,
                        productId = %product_id,
                        error = %err,
                        r#type = %std::any::type_name::<NotificationRecord>(),
                        "Failed deserializing."
                    );
                    None
                }
            })
            .collect();

        Ok(records)
    }
}
