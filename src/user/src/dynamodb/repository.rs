use crate::dynamodb::user_record::{UserRecord, mk_pk, mk_sk};
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
use common::user_id::UserId;
use tracing::error;

#[async_trait::async_trait]
#[mockall::automock]
pub trait UserDynamoDbRepository {
    async fn get_user_record(
        &self,
        user_id: &UserId,
    ) -> Result<Option<UserRecord>, SdkError<GetItemError, HttpResponse>>;

    async fn put_user_record(
        &self,
        user_record: UserRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError, HttpResponse>>;
}

#[derive(Debug, Clone)]
pub struct UserDynamoDbRepositoryImpl<'a> {
    client: &'a Client,
    table: String,
}

impl<'a> UserDynamoDbRepositoryImpl<'a> {
    pub fn new(client: &'a Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }
}

#[async_trait::async_trait]
impl<'a> UserDynamoDbRepository for UserDynamoDbRepositoryImpl<'a> {
    async fn get_user_record(
        &self,
        user_id: &UserId,
    ) -> Result<Option<UserRecord>, SdkError<GetItemError, HttpResponse>> {
        let rec = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk().to_owned()))
            .send()
            .await?
            .item
            .map(serde_dynamo::from_item::<_, UserRecord>)
            .and_then(|user_record_res| match user_record_res {
                Ok(item_record) => Some(item_record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<UserRecord>(), "Failed deserializing.");
                    None
                }
            });

        Ok(rec)
    }

    async fn put_user_record(
        &self,
        user_record: UserRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError, HttpResponse>> {
        let payload = serde_dynamo::to_item(user_record).map_err(SdkError::construction_failure)?;
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(payload))
            .send()
            .await
    }
}
