use crate::core::authorization_code::OAuthAuthorizationCode;
use crate::core::client::OAuthClientId;
use crate::dynamodb::authorization_code_record::{self, AuthorizationCodeRecord};
use crate::dynamodb::client_record::{self, OAuthClientRecord};
use aws_sdk_dynamodb::{
    Client,
    error::SdkError,
    operation::{
        delete_item::{DeleteItemError, DeleteItemOutput},
        get_item::GetItemError,
        put_item::{PutItemError, PutItemOutput},
    },
    types::AttributeValue,
};
use tracing::error;

#[async_trait::async_trait]
#[mockall::automock]
pub trait OAuthRepository {
    async fn get_client_record(
        &self,
        client_id: &OAuthClientId,
    ) -> Result<Option<OAuthClientRecord>, SdkError<GetItemError>>;

    async fn put_client_record(
        &self,
        record: OAuthClientRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError>>;

    async fn put_authorization_code_record(
        &self,
        record: AuthorizationCodeRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError>>;

    async fn get_authorization_code_record(
        &self,
        code: &OAuthAuthorizationCode,
    ) -> Result<Option<AuthorizationCodeRecord>, SdkError<GetItemError>>;

    async fn delete_authorization_code_record(
        &self,
        code: &OAuthAuthorizationCode,
    ) -> Result<DeleteItemOutput, SdkError<DeleteItemError>>;
}

#[derive(Debug, Clone)]
pub struct OAuthDynamoDbRepositoryImpl<'a> {
    client: &'a Client,
    table: String,
}

impl<'a> OAuthDynamoDbRepositoryImpl<'a> {
    pub fn new(client: &'a Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }
}

#[async_trait::async_trait]
impl OAuthRepository for OAuthDynamoDbRepositoryImpl<'_> {
    async fn get_client_record(
        &self,
        client_id: &OAuthClientId,
    ) -> Result<Option<OAuthClientRecord>, SdkError<GetItemError>> {
        let rec = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(client_record::mk_pk(client_id)))
            .key("sk", AttributeValue::S(client_record::mk_sk().to_owned()))
            .send()
            .await?
            .item
            .map(serde_dynamo::from_item::<_, OAuthClientRecord>)
            .and_then(|record_res| match record_res {
                Ok(record) => Some(record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<OAuthClientRecord>(), "Failed deserializing OAuthClientRecord.");
                    None
                }
            });
        Ok(rec)
    }

    async fn put_client_record(
        &self,
        record: OAuthClientRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError>> {
        let payload = serde_dynamo::to_item(record).map_err(SdkError::construction_failure)?;
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(payload))
            .send()
            .await
    }

    async fn put_authorization_code_record(
        &self,
        record: AuthorizationCodeRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError>> {
        let payload = serde_dynamo::to_item(record).map_err(SdkError::construction_failure)?;
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(payload))
            .send()
            .await
    }

    async fn get_authorization_code_record(
        &self,
        code: &OAuthAuthorizationCode,
    ) -> Result<Option<AuthorizationCodeRecord>, SdkError<GetItemError>> {
        let rec = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(authorization_code_record::mk_pk(code)))
            .key(
                "sk",
                AttributeValue::S(authorization_code_record::mk_sk().to_owned()),
            )
            .send()
            .await?
            .item
            .map(serde_dynamo::from_item::<_, AuthorizationCodeRecord>)
            .and_then(|record_res| match record_res {
                Ok(record) => Some(record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<AuthorizationCodeRecord>(), "Failed deserializing AuthorizationCodeRecord.");
                    None
                }
            });
        Ok(rec)
    }

    async fn delete_authorization_code_record(
        &self,
        code: &OAuthAuthorizationCode,
    ) -> Result<DeleteItemOutput, SdkError<DeleteItemError>> {
        self.client
            .delete_item()
            .table_name(&self.table)
            .key(
                "pk",
                AttributeValue::S(authorization_code_record::mk_pk(code)),
            )
            .key(
                "sk",
                AttributeValue::S(authorization_code_record::mk_sk().to_owned()),
            )
            .send()
            .await
    }
}
