use crate::dynamodb::record::{FxRatesRecord, mk_pk, mk_sk};
use aws_sdk_dynamodb::{
    Client,
    error::SdkError,
    operation::{
        get_item::GetItemError,
        put_item::{PutItemError, PutItemOutput},
    },
    types::AttributeValue,
};
use tracing::error;

#[async_trait::async_trait]
#[mockall::automock]
pub trait FxRateDynamoDbRepository {
    async fn put_fx_rates_record(
        &self,
        record: FxRatesRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError>>;

    async fn get_fx_rates_record(&self) -> Result<Option<FxRatesRecord>, SdkError<GetItemError>>;
}

#[derive(Debug, Clone)]
pub struct FxRateDynamoDbRepositoryImpl<'a> {
    client: &'a Client,
    table: String,
}

impl<'a> FxRateDynamoDbRepositoryImpl<'a> {
    pub fn new(client: &'a Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }
}

#[async_trait::async_trait]
impl<'a> FxRateDynamoDbRepository for FxRateDynamoDbRepositoryImpl<'a> {
    async fn put_fx_rates_record(
        &self,
        record: FxRatesRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError>> {
        let payload = serde_dynamo::to_item(record).map_err(SdkError::construction_failure)?;
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(payload))
            .send()
            .await
    }

    async fn get_fx_rates_record(&self) -> Result<Option<FxRatesRecord>, SdkError<GetItemError>> {
        let rec = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk().to_owned()))
            .key("sk", AttributeValue::S(mk_sk().to_owned()))
            .send()
            .await?
            .item
            .map(serde_dynamo::from_item::<_, FxRatesRecord>)
            .and_then(|record_res| match record_res {
                Ok(record) => Some(record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<FxRatesRecord>(), "Failed deserializing.");
                    None
                }
            });

        Ok(rec)
    }
}
