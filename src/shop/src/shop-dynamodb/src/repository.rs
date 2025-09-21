use crate::shop_record::{ShopRecord, mk_pk};
use aws_sdk_dynamodb::{
    Client,
    config::http::HttpResponse,
    error::SdkError,
    operation::{
        get_item::GetItemError,
        put_item::{PutItemError, PutItemOutput},
    },
    types::AttributeValue,
};
use common::shop_id::ShopId;
use tracing::error;

#[async_trait::async_trait]
#[mockall::automock]
pub trait ShopDynamoDbRepository {
    async fn put_shop_record(
        &self,
        record: ShopRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError, HttpResponse>>;

    async fn get_shop_record(
        &self,
        shop_id: &ShopId,
    ) -> Result<Option<ShopRecord>, SdkError<GetItemError, HttpResponse>>;
}

#[derive(Debug, Clone)]
pub struct ShopDynamoDbRepositoryImpl<'a> {
    client: &'a Client,
    table: String,
}

impl<'a> ShopDynamoDbRepositoryImpl<'a> {
    pub fn new(client: &'a Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }
}

#[async_trait::async_trait]
impl<'a> ShopDynamoDbRepository for ShopDynamoDbRepositoryImpl<'a> {
    async fn put_shop_record(
        &self,
        record: ShopRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError, HttpResponse>> {
        let payload = serde_dynamo::to_item(record).map_err(SdkError::construction_failure)?;
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(payload))
            .send()
            .await
    }

    async fn get_shop_record(
        &self,
        shop_id: &ShopId,
    ) -> Result<Option<ShopRecord>, SdkError<GetItemError, HttpResponse>> {
        let rec = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(shop_id)))
            .key("sk", AttributeValue::S("shop#details".to_owned()))
            .send()
            .await?
            .item
            .map(serde_dynamo::from_item::<_, ShopRecord>)
            .and_then(|item_record_res| match item_record_res {
                Ok(item_record) => Some(item_record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<ShopRecord>(), "Failed deserializing.");
                    None
                }
            });

        Ok(rec)
    }
}
