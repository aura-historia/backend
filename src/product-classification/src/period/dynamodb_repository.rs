use crate::period::record::{PeriodRecord, mk_pk, mk_sk};
use aws_sdk_dynamodb::{
    Client,
    error::SdkError,
    operation::{
        get_item::GetItemError,
        put_item::{PutItemError, PutItemOutput},
        query::QueryError,
    },
    types::AttributeValue,
};
use common::period_key::PeriodId;
use tracing::error;

#[async_trait::async_trait]
#[mockall::automock]
pub trait PeriodDynamoDbRepository {
    async fn put_period_record(
        &self,
        record: PeriodRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError>>;

    async fn get_period_record(
        &self,
        period_id: &PeriodId,
    ) -> Result<Option<PeriodRecord>, SdkError<GetItemError>>;

    async fn query_period_records(&self) -> Result<Vec<PeriodRecord>, SdkError<QueryError>>;
}

#[derive(Debug, Clone)]
pub struct PeriodDynamoDbRepositoryImpl<'a> {
    client: &'a Client,
    table: String,
}

impl<'a> PeriodDynamoDbRepositoryImpl<'a> {
    pub fn new(client: &'a Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }
}

#[async_trait::async_trait]
impl<'a> PeriodDynamoDbRepository for PeriodDynamoDbRepositoryImpl<'a> {
    async fn put_period_record(
        &self,
        record: PeriodRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError>> {
        let payload = serde_dynamo::to_item(record).map_err(SdkError::construction_failure)?;
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(payload))
            .send()
            .await
    }

    async fn get_period_record(
        &self,
        period_id: &PeriodId,
    ) -> Result<Option<PeriodRecord>, SdkError<GetItemError>> {
        let rec = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk().to_owned()))
            .key("sk", AttributeValue::S(mk_sk(period_id)))
            .send()
            .await?
            .item
            .map(serde_dynamo::from_item::<_, PeriodRecord>)
            .and_then(|period_record_res| match period_record_res {
                Ok(period_record) => Some(period_record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<PeriodRecord>(), "Failed deserializing.");
                    None
                }
            });

        Ok(rec)
    }

    async fn query_period_records(&self) -> Result<Vec<PeriodRecord>, SdkError<QueryError>> {
        let records = self.client
            .query()
            .table_name(&self.table)
            .key_condition_expression("#pk = :pk_val AND begins_with(#sk, :sk_prefix)")
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#sk", "sk")
            .expression_attribute_values(
                ":pk_val",
                AttributeValue::S(mk_pk().to_owned()),
            )
            .expression_attribute_values(":sk_prefix", AttributeValue::S("period#".to_string()))
            .send()
            .await?
            .items
            .unwrap_or_default()
            .into_iter()
            .map(serde_dynamo::from_item::<_, PeriodRecord>)
            .filter_map(|res| match res {
                Ok(record) => Some(record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<PeriodRecord>(), "Failed deserializing.");
                    None
                }
            })
            .collect();

        Ok(records)
    }
}
