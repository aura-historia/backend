use crate::{
    mail_id::MailId,
    record::{MailRecord, mk_pk, mk_sk},
};
use aws_sdk_dynamodb::{
    Client,
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
pub trait MailDynamoDbRepository {
    async fn get_mail_record(
        &self,
        user_id: &UserId,
        mail_id: &MailId,
    ) -> Result<Option<MailRecord>, SdkError<GetItemError>>;

    async fn put_mail_record(
        &self,
        mail_record: MailRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError>>;
}

#[derive(Debug, Clone)]
pub struct MailDynamoDbRepositoryImpl<'a> {
    client: &'a Client,
    table: String,
}

impl<'a> MailDynamoDbRepositoryImpl<'a> {
    pub fn new(client: &'a Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }
}

#[async_trait::async_trait]
impl<'a> MailDynamoDbRepository for MailDynamoDbRepositoryImpl<'a> {
    async fn get_mail_record(
        &self,
        user_id: &UserId,
        mail_id: &MailId,
    ) -> Result<Option<MailRecord>, SdkError<GetItemError>> {
        let rec = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk(mail_id)))
            .send()
            .await?
            .item
            .map(serde_dynamo::from_item::<_, MailRecord>)
            .and_then(|mail_record_res| match mail_record_res {
                Ok(mail_record) => Some(mail_record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<MailRecord>(), "Failed deserializing.");
                    None
                }
            });

        Ok(rec)
    }

    async fn put_mail_record(
        &self,
        mail_record: MailRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError>> {
        let payload = serde_dynamo::to_item(mail_record).map_err(SdkError::construction_failure)?;
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(payload))
            .send()
            .await
    }
}
