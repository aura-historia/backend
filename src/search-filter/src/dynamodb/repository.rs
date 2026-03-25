use crate::core::user_search_filter_id::UserSearchFilterId;
use crate::dynamodb::user_search_filter_match_record::UserSearchFilterMatchRecord;
use crate::dynamodb::user_search_filter_record::{UserSearchFilterRecord, mk_pk, mk_sk};
use crate::dynamodb::user_search_filter_record_update::UserSearchFilterRecordUpdate;
use aws_sdk_dynamodb::{
    Client,
    config::http::HttpResponse,
    error::SdkError,
    operation::{
        batch_write_item::{BatchWriteItemError, BatchWriteItemOutput},
        delete_item::{DeleteItemError, DeleteItemOutput},
        get_item::GetItemError,
        put_item::{PutItemError, PutItemOutput},
        query::QueryError,
        update_item::UpdateItemError,
    },
    types::{AttributeValue, ReturnValue},
};
use common::{
    batch::Batch, dynamodb_update::DynamoDbUpdate, pagination::cursor::Cursor, shop_id::ShopId,
    shops_product_id::ShopsProductId, user_id::UserId,
};
use std::collections::HashMap;
use time::{Duration, OffsetDateTime};
use tracing::error;

#[async_trait::async_trait]
#[mockall::automock]
pub trait UserSearchFilterDynamoDbRepository {
    async fn query_user_search_filter_records(
        &self,
        user_id: &UserId,
        scan_index_forward: bool,
    ) -> Result<Vec<UserSearchFilterRecord>, SdkError<QueryError, HttpResponse>>;

    async fn get_user_search_filter_record(
        &self,
        user_id: &UserId,
        search_filter_id: &UserSearchFilterId,
    ) -> Result<Option<UserSearchFilterRecord>, SdkError<GetItemError, HttpResponse>>;

    async fn put_user_search_filter_record(
        &self,
        record: UserSearchFilterRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError, HttpResponse>>;

    async fn delete_user_search_filter_record(
        &self,
        user_id: &UserId,
        search_filter_id: &UserSearchFilterId,
    ) -> Result<DeleteItemOutput, SdkError<DeleteItemError, HttpResponse>>;

    async fn update_user_search_filter_record(
        &self,
        user_id: &UserId,
        search_filter_id: &UserSearchFilterId,
        search_filter_update: UserSearchFilterRecordUpdate,
    ) -> Result<Option<UserSearchFilterRecord>, SdkError<UpdateItemError, HttpResponse>>;

    async fn get_user_search_filter_match_record(
        &self,
        user_id: &UserId,
        search_filter_id: &UserSearchFilterId,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<Option<UserSearchFilterMatchRecord>, SdkError<GetItemError, HttpResponse>>;

    async fn query_user_search_filter_match_records_all(
        &self,
        user_id: &UserId,
    ) -> Result<Vec<UserSearchFilterMatchRecord>, SdkError<QueryError, HttpResponse>>;

    async fn query_user_search_filter_match_records_for_filter(
        &self,
        user_id: &UserId,
        search_filter_id: &UserSearchFilterId,
        created_cursor: Option<Cursor<OffsetDateTime>>,
        scan_index_forward: bool,
    ) -> Result<Vec<UserSearchFilterMatchRecord>, SdkError<QueryError, HttpResponse>>;

    async fn count_user_search_filter_match_records_for_filter(
        &self,
        user_id: &UserId,
        search_filter_id: &UserSearchFilterId,
        created_cursor: Option<Cursor<OffsetDateTime>>,
        scan_index_forward: bool,
    ) -> Result<u64, SdkError<QueryError, HttpResponse>>;

    async fn put_user_search_filter_match_record(
        &self,
        record: UserSearchFilterMatchRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError, HttpResponse>>;

    async fn put_user_search_filter_match_records(
        &self,
        records: Batch<UserSearchFilterMatchRecord, 25>,
    ) -> Result<BatchWriteItemOutput, SdkError<BatchWriteItemError, HttpResponse>>;
}

#[derive(Debug, Clone)]
pub struct UserSearchFilterDynamoDbRepositoryImpl<'a> {
    client: &'a Client,
    table: String,
}

impl<'a> UserSearchFilterDynamoDbRepositoryImpl<'a> {
    pub fn new(client: &'a Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }
}

#[allow(clippy::result_large_err)]
fn compute_lsi1_sk_bounds(
    cursor: &Cursor<OffsetDateTime>,
    scan_index_forward: bool,
) -> Result<(String, String), SdkError<QueryError, HttpResponse>> {
    use crate::dynamodb::user_search_filter_match_record as match_record;

    if scan_index_forward {
        let lower = match cursor.search_after {
            Some(created) => match_record::mk_lsi1_sk(&(created + Duration::NANOSECOND))
                .map_err(SdkError::construction_failure)?,
            None => match_record::LSI1_SK_LOWER_BOUND.to_string(),
        };
        Ok((lower, match_record::LSI1_SK_UPPER_BOUND.to_string()))
    } else {
        let upper = match cursor.search_after {
            Some(created) => match_record::mk_lsi1_sk(&(created - Duration::NANOSECOND))
                .map_err(SdkError::construction_failure)?,
            None => match_record::LSI1_SK_UPPER_BOUND.to_string(),
        };
        Ok((match_record::LSI1_SK_LOWER_BOUND.to_string(), upper))
    }
}

#[async_trait::async_trait]
impl<'a> UserSearchFilterDynamoDbRepository for UserSearchFilterDynamoDbRepositoryImpl<'a> {
    async fn query_user_search_filter_records(
        &self,
        user_id: &UserId,
        scan_index_forward: bool,
    ) -> Result<Vec<UserSearchFilterRecord>, SdkError<QueryError, HttpResponse>> {
        let records = self.client
            .query()
            .table_name(&self.table)
            .key_condition_expression("#pk = :pk_val AND begins_with(#sk, :sk_prefix)")
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#sk", "sk")
            .expression_attribute_values(":pk_val", AttributeValue::S(mk_pk(user_id)))
            .expression_attribute_values(
                ":sk_prefix",
                AttributeValue::S("search_filter#".to_string()),
            )
            .scan_index_forward(scan_index_forward)
            .into_paginator()
            .send()
            .try_collect()
            .await?
            .into_iter()
            .flat_map(|qo| qo.items.unwrap_or_default())
            .map(serde_dynamo::from_item::<_, UserSearchFilterRecord>)
            .filter_map(|result| match result {
                Ok(search_filter_record) => Some(search_filter_record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<UserSearchFilterRecord>(), "Failed deserializing SearchFilterRecord.");
                    None
                }
            })
            .collect::<Vec<_>>();
        Ok(records)
    }

    async fn get_user_search_filter_record(
        &self,
        user_id: &UserId,
        search_filter_id: &UserSearchFilterId,
    ) -> Result<Option<UserSearchFilterRecord>, SdkError<GetItemError, HttpResponse>> {
        let record = self.client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk(search_filter_id)))
            .send()
            .await?
            .item
            .map(serde_dynamo::from_item::<_, UserSearchFilterRecord>)
            .and_then(|record_res| match record_res {
                Ok(search_filter_record) => Some(search_filter_record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<UserSearchFilterRecord>(), "Failed deserializing SearchFilterRecord.");
                    None
                }
            });
        Ok(record)
    }

    async fn put_user_search_filter_record(
        &self,
        record: UserSearchFilterRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError, HttpResponse>> {
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(
                serde_dynamo::to_item(record).map_err(SdkError::construction_failure)?,
            ))
            .send()
            .await
    }

    async fn delete_user_search_filter_record(
        &self,
        user_id: &UserId,
        search_filter_id: &UserSearchFilterId,
    ) -> Result<DeleteItemOutput, SdkError<DeleteItemError, HttpResponse>> {
        self.client
            .delete_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk(search_filter_id)))
            .send()
            .await
    }

    async fn update_user_search_filter_record(
        &self,
        user_id: &UserId,
        search_filter_id: &UserSearchFilterId,
        search_filter_update: UserSearchFilterRecordUpdate,
    ) -> Result<Option<UserSearchFilterRecord>, SdkError<UpdateItemError, HttpResponse>> {
        let pk = mk_pk(user_id);
        let sk = mk_sk(search_filter_id);
        let update_expr = search_filter_update.into_update_expr()?;

        self.client
            .update_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(pk))
            .key("sk", AttributeValue::S(sk))
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
                    error!(error = %err, type = %std::any::type_name::<UserSearchFilterRecord>(), "Failed deserializing SearchFilterRecord.");
                    None
                }
            }))
    }

    async fn get_user_search_filter_match_record(
        &self,
        user_id: &UserId,
        search_filter_id: &UserSearchFilterId,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<Option<UserSearchFilterMatchRecord>, SdkError<GetItemError, HttpResponse>> {
        use crate::dynamodb::user_search_filter_match_record as match_record;
        let record = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(match_record::mk_pk(user_id)))
            .key(
                "sk",
                AttributeValue::S(match_record::mk_sk(
                    search_filter_id,
                    shop_id,
                    shops_product_id,
                )),
            )
            .send()
            .await?
            .item
            .map(serde_dynamo::from_item::<_, UserSearchFilterMatchRecord>)
            .and_then(|record_res| match record_res {
                Ok(match_record) => Some(match_record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<UserSearchFilterMatchRecord>(), "Failed deserializing UserSearchFilterMatchRecord.");
                    None
                }
            });
        Ok(record)
    }

    async fn query_user_search_filter_match_records_all(
        &self,
        user_id: &UserId,
    ) -> Result<Vec<UserSearchFilterMatchRecord>, SdkError<QueryError, HttpResponse>> {
        use crate::dynamodb::user_search_filter_match_record as match_record;
        let records = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("#pk = :pk_val AND begins_with(#sk, :sk_prefix)")
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#sk", "sk")
            .expression_attribute_values(
                ":pk_val",
                AttributeValue::S(match_record::mk_pk(user_id)),
            )
            .expression_attribute_values(
                ":sk_prefix",
                AttributeValue::S(match_record::mk_sk_prefix_all().to_string()),
            )
            .into_paginator()
            .send()
            .try_collect()
            .await?
            .into_iter()
            .flat_map(|qo| qo.items.unwrap_or_default())
            .map(serde_dynamo::from_item::<_, UserSearchFilterMatchRecord>)
            .filter_map(|result| match result {
                Ok(record) => Some(record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<UserSearchFilterMatchRecord>(), "Failed deserializing UserSearchFilterMatchRecord.");
                    None
                }
            })
            .collect::<Vec<_>>();
        Ok(records)
    }

    async fn query_user_search_filter_match_records_for_filter(
        &self,
        user_id: &UserId,
        search_filter_id: &UserSearchFilterId,
        created_cursor: Option<Cursor<OffsetDateTime>>,
        scan_index_forward: bool,
    ) -> Result<Vec<UserSearchFilterMatchRecord>, SdkError<QueryError, HttpResponse>> {
        use crate::dynamodb::user_search_filter_match_record as match_record;

        match created_cursor {
            Some(cursor) => {
                let (lsi1_sk_lower, lsi1_sk_upper) =
                    compute_lsi1_sk_bounds(&cursor, scan_index_forward)?;

                let records = self
                    .client
                    .query()
                    .table_name(&self.table)
                    .index_name("lsi1")
                    .key_condition_expression(
                        "#pk = :pk_val AND #lsi1_sk BETWEEN :lsi1_sk_lower AND :lsi1_sk_upper",
                    )
                    .filter_expression("begins_with(#sk, :sk_prefix)")
                    .expression_attribute_names("#pk", "pk")
                    .expression_attribute_names("#lsi1_sk", "lsi1_sk")
                    .expression_attribute_names("#sk", "sk")
                    .expression_attribute_values(
                        ":pk_val",
                        AttributeValue::S(match_record::mk_pk(user_id)),
                    )
                    .expression_attribute_values(
                        ":lsi1_sk_lower",
                        AttributeValue::S(lsi1_sk_lower),
                    )
                    .expression_attribute_values(
                        ":lsi1_sk_upper",
                        AttributeValue::S(lsi1_sk_upper),
                    )
                    .expression_attribute_values(
                        ":sk_prefix",
                        AttributeValue::S(match_record::mk_sk_prefix_filter(search_filter_id)),
                    )
                    .limit(cursor.size as i32)
                    .scan_index_forward(scan_index_forward)
                    .send()
                    .await?
                    .items
                    .unwrap_or_default()
                    .into_iter()
                    .map(serde_dynamo::from_item::<_, UserSearchFilterMatchRecord>)
                    .filter_map(|result| match result {
                        Ok(record) => Some(record),
                        Err(err) => {
                            error!(error = %err, type = %std::any::type_name::<UserSearchFilterMatchRecord>(), "Failed deserializing UserSearchFilterMatchRecord.");
                            None
                        }
                    })
                    .collect::<Vec<_>>();
                Ok(records)
            }
            None => {
                let records = self
                    .client
                    .query()
                    .table_name(&self.table)
                    .key_condition_expression("#pk = :pk_val AND begins_with(#sk, :sk_prefix)")
                    .expression_attribute_names("#pk", "pk")
                    .expression_attribute_names("#sk", "sk")
                    .expression_attribute_values(
                        ":pk_val",
                        AttributeValue::S(match_record::mk_pk(user_id)),
                    )
                    .expression_attribute_values(
                        ":sk_prefix",
                        AttributeValue::S(match_record::mk_sk_prefix_filter(search_filter_id)),
                    )
                    .scan_index_forward(scan_index_forward)
                    .into_paginator()
                    .send()
                    .try_collect()
                    .await?
                    .into_iter()
                    .flat_map(|qo| qo.items.unwrap_or_default())
                    .map(serde_dynamo::from_item::<_, UserSearchFilterMatchRecord>)
                    .filter_map(|result| match result {
                        Ok(record) => Some(record),
                        Err(err) => {
                            error!(error = %err, type = %std::any::type_name::<UserSearchFilterMatchRecord>(), "Failed deserializing UserSearchFilterMatchRecord.");
                            None
                        }
                    })
                    .collect::<Vec<_>>();
                Ok(records)
            }
        }
    }

    async fn count_user_search_filter_match_records_for_filter(
        &self,
        user_id: &UserId,
        search_filter_id: &UserSearchFilterId,
        created_cursor: Option<Cursor<OffsetDateTime>>,
        scan_index_forward: bool,
    ) -> Result<u64, SdkError<QueryError, HttpResponse>> {
        use crate::dynamodb::user_search_filter_match_record as match_record;

        match created_cursor {
            Some(cursor) => {
                let (lsi1_sk_lower, lsi1_sk_upper) =
                    compute_lsi1_sk_bounds(&cursor, scan_index_forward)?;

                let count = self
                    .client
                    .query()
                    .table_name(&self.table)
                    .index_name("lsi1")
                    .key_condition_expression(
                        "#pk = :pk_val AND #lsi1_sk BETWEEN :lsi1_sk_lower AND :lsi1_sk_upper",
                    )
                    .filter_expression("begins_with(#sk, :sk_prefix)")
                    .expression_attribute_names("#pk", "pk")
                    .expression_attribute_names("#lsi1_sk", "lsi1_sk")
                    .expression_attribute_names("#sk", "sk")
                    .expression_attribute_values(
                        ":pk_val",
                        AttributeValue::S(match_record::mk_pk(user_id)),
                    )
                    .expression_attribute_values(":lsi1_sk_lower", AttributeValue::S(lsi1_sk_lower))
                    .expression_attribute_values(":lsi1_sk_upper", AttributeValue::S(lsi1_sk_upper))
                    .expression_attribute_values(
                        ":sk_prefix",
                        AttributeValue::S(match_record::mk_sk_prefix_filter(search_filter_id)),
                    )
                    .scan_index_forward(scan_index_forward)
                    .select(aws_sdk_dynamodb::types::Select::Count)
                    .send()
                    .await?
                    .count;

                Ok(count as u64)
            }
            None => {
                let items = self
                    .client
                    .query()
                    .table_name(&self.table)
                    .key_condition_expression("#pk = :pk_val AND begins_with(#sk, :sk_prefix)")
                    .expression_attribute_names("#pk", "pk")
                    .expression_attribute_names("#sk", "sk")
                    .expression_attribute_values(
                        ":pk_val",
                        AttributeValue::S(match_record::mk_pk(user_id)),
                    )
                    .expression_attribute_values(
                        ":sk_prefix",
                        AttributeValue::S(match_record::mk_sk_prefix_filter(search_filter_id)),
                    )
                    .scan_index_forward(scan_index_forward)
                    .select(aws_sdk_dynamodb::types::Select::Count)
                    .into_paginator()
                    .send()
                    .try_collect()
                    .await?
                    .into_iter()
                    .map(|qo| qo.count as u64)
                    .sum();

                Ok(items)
            }
        }
    }

    async fn put_user_search_filter_match_record(
        &self,
        record: UserSearchFilterMatchRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError, HttpResponse>> {
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(
                serde_dynamo::to_item(record).map_err(SdkError::construction_failure)?,
            ))
            .send()
            .await
    }

    async fn put_user_search_filter_match_records(
        &self,
        records: Batch<UserSearchFilterMatchRecord, 25>,
    ) -> Result<BatchWriteItemOutput, SdkError<BatchWriteItemError, HttpResponse>> {
        self.client
            .batch_write_item()
            .set_request_items(Some(HashMap::from([(
                self.table.clone(),
                records.into_dynamodb_write_requests(),
            )])))
            .send()
            .await
    }
}
