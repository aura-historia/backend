use crate::{
    record::{WatchlistItemRecord, mk_pk, mk_sk},
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
use common::{dynamodb_update::mk_update, pagination::cursor::Cursor, user_id::UserId};
use time::{OffsetDateTime, macros::datetime};
use tracing::error;

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
        created: &OffsetDateTime,
    ) -> Result<Option<WatchlistItemRecord>, SdkError<GetItemError>>;

    async fn delete_watchlist_record(
        &self,
        user_id: &UserId,
        created: &OffsetDateTime,
    ) -> Result<DeleteItemOutput, SdkError<DeleteItemError>>;

    async fn update_watchlist_record(
        &self,
        user_id: &UserId,
        created: &OffsetDateTime,
        update: WatchlistItemRecordUpdate,
    ) -> Result<Option<WatchlistItemRecord>, SdkError<UpdateItemError>>;
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
            "#pk = :pk_val AND #sk > :sk_val_exclusive_guard"
        } else {
            "#pk = :pk_val AND #sk < :sk_val_exclusive_guard"
        };

        let records = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression(key_condition_expression)
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#sk", "sk")
            .expression_attribute_values(
                ":pk_val",
                AttributeValue::S(mk_pk(user_id)),
            )
            .expression_attribute_values(
                ":sk_val_exclusive_guard",
                AttributeValue::S(mk_sk(&exclusive_guard).map_err(SdkError::construction_failure)?),
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
        created: &OffsetDateTime,
    ) -> Result<Option<WatchlistItemRecord>, SdkError<GetItemError>> {
        let pk = mk_pk(user_id);
        let sk = mk_sk(created).map_err(SdkError::construction_failure)?;
        let record = self.client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(pk))
            .key("sk", AttributeValue::S(sk))
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
        created: &OffsetDateTime,
    ) -> Result<DeleteItemOutput, SdkError<DeleteItemError>> {
        let pk = mk_pk(user_id);
        let sk = mk_sk(created).map_err(SdkError::construction_failure)?;
        self.client
            .delete_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(pk))
            .key("sk", AttributeValue::S(sk))
            .send()
            .await
    }

    async fn update_watchlist_record(
        &self,
        user_id: &UserId,
        created: &OffsetDateTime,
        update: WatchlistItemRecordUpdate,
    ) -> Result<Option<WatchlistItemRecord>, SdkError<UpdateItemError>> {
        let pk = mk_pk(user_id);
        let sk = mk_sk(created).map_err(SdkError::construction_failure)?;
        let update_expr = mk_update(update)?;

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
                Ok(watchlist_record) => Some(watchlist_record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<WatchlistItemRecord>(), "Failed deserializing WatchlistItemRecord.");
                    None
                }
            }))
    }
}
