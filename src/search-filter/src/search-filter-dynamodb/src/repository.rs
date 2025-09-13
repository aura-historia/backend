use crate::{
    search_filter_record::{SearchFilterRecord, mk_pk, mk_sk},
    search_filter_record_update::SearchFilterRecordUpdate,
};
use aws_sdk_dynamodb::{
    Client,
    config::http::HttpResponse,
    error::SdkError,
    operation::{
        delete_item::{DeleteItemError, DeleteItemOutput},
        get_item::GetItemError,
        put_item::{PutItemError, PutItemOutput},
        query::QueryError,
        update_item::{UpdateItemError, UpdateItemOutput},
    },
    types::AttributeValue,
};
use common::{dynamodb_update::mk_update, user_id::UserId};
use search_filter_core::search_filter_id::SearchFilterId;
use tracing::error;

#[async_trait::async_trait]
#[mockall::automock]
pub trait SearchFilterDynamoDbRepository {
    async fn query_search_filter_records(
        &self,
        user_id: &UserId,
        scan_index_forward: bool,
    ) -> Result<Vec<SearchFilterRecord>, SdkError<QueryError, HttpResponse>>;

    async fn get_search_filter_record(
        &self,
        user_id: &UserId,
        search_filter_id: &SearchFilterId,
    ) -> Result<Option<SearchFilterRecord>, SdkError<GetItemError, HttpResponse>>;

    async fn put_search_filter_record(
        &self,
        record: SearchFilterRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError, HttpResponse>>;

    async fn delete_search_filter_record(
        &self,
        user_id: &UserId,
        search_filter_id: &SearchFilterId,
    ) -> Result<DeleteItemOutput, SdkError<DeleteItemError, HttpResponse>>;

    async fn update_search_filter_record(
        &self,
        user_id: &UserId,
        search_filter_id: &SearchFilterId,
        search_filter_update: SearchFilterRecordUpdate,
    ) -> Result<UpdateItemOutput, SdkError<UpdateItemError, HttpResponse>>;
}

#[derive(Debug, Clone)]
pub struct SearchFilterDynamoDbRepositoryImpl<'a> {
    client: &'a Client,
    table: String,
}

impl<'a> SearchFilterDynamoDbRepositoryImpl<'a> {
    pub fn new(client: &'a Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }
}

#[async_trait::async_trait]
impl<'a> SearchFilterDynamoDbRepository for SearchFilterDynamoDbRepositoryImpl<'a> {
    async fn query_search_filter_records(
        &self,
        user_id: &UserId,
        scan_index_forward: bool,
    ) -> Result<Vec<SearchFilterRecord>, SdkError<QueryError, HttpResponse>> {
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
            .map(serde_dynamo::from_item::<_, SearchFilterRecord>)
            .filter_map(|result| match result {
                Ok(search_filter_record) => Some(search_filter_record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<SearchFilterRecord>(), "Failed deserializing SearchFilterRecord.");
                    None
                }
            })
            .collect::<Vec<_>>();
        Ok(records)
    }

    async fn get_search_filter_record(
        &self,
        user_id: &UserId,
        search_filter_id: &SearchFilterId,
    ) -> Result<Option<SearchFilterRecord>, SdkError<GetItemError, HttpResponse>> {
        let record = self.client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk(search_filter_id)))
            .send()
            .await?
            .item
            .map(serde_dynamo::from_item::<_, SearchFilterRecord>)
            .and_then(|item_record_res| match item_record_res {
                Ok(search_filter_record) => Some(search_filter_record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<SearchFilterRecord>(), "Failed deserializing SearchFilterRecord.");
                    None
                }
            });
        Ok(record)
    }

    async fn put_search_filter_record(
        &self,
        record: SearchFilterRecord,
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

    async fn delete_search_filter_record(
        &self,
        user_id: &UserId,
        search_filter_id: &SearchFilterId,
    ) -> Result<DeleteItemOutput, SdkError<DeleteItemError, HttpResponse>> {
        self.client
            .delete_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk(search_filter_id)))
            .send()
            .await
    }

    async fn update_search_filter_record(
        &self,
        user_id: &UserId,
        search_filter_id: &SearchFilterId,
        search_filter_update: SearchFilterRecordUpdate,
    ) -> Result<UpdateItemOutput, SdkError<UpdateItemError, HttpResponse>> {
        let pk = mk_pk(user_id);
        let sk = mk_sk(search_filter_id);
        let update_expr = mk_update(search_filter_update)?;

        self.client
            .update_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(pk))
            .key("sk", AttributeValue::S(sk))
            .update_expression(update_expr.update_expr)
            .set_expression_attribute_names(Some(update_expr.expr_attr_names))
            .set_expression_attribute_values(Some(update_expr.expr_attr_values))
            .send()
            .await
    }
}
