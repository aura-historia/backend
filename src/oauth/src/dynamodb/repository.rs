use crate::core::authorization_code::OAuthAuthorizationCode;
use crate::dynamodb::authorization_code_record::{self, AuthorizationCodeRecord};
use crate::dynamodb::client_record::{self, OAuthClientRecord};
use crate::dynamodb::client_record_update::OAuthClientRecordUpdate;
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
use common::dynamodb_update::DynamoDbUpdate;
use common::oauth_client_id::OAuthClientId;
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

    async fn query_client_records(&self) -> Result<Vec<OAuthClientRecord>, SdkError<QueryError>>;

    async fn update_client_record(
        &self,
        client_id: &OAuthClientId,
        update: OAuthClientRecordUpdate,
    ) -> Result<Option<OAuthClientRecord>, SdkError<UpdateItemError>>;

    async fn delete_client_record(
        &self,
        client_id: &OAuthClientId,
    ) -> Result<DeleteItemOutput, SdkError<DeleteItemError>>;

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
            .key("pk", AttributeValue::S(client_record::mk_pk().to_owned()))
            .key("sk", AttributeValue::S(client_record::mk_sk(client_id)))
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

    async fn query_client_records(&self) -> Result<Vec<OAuthClientRecord>, SdkError<QueryError>> {
        let items = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("#pk = :pk_val AND begins_with(#sk, :sk_prefix)")
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#sk", "sk")
            .expression_attribute_values(
                ":pk_val",
                AttributeValue::S(client_record::mk_pk().to_owned()),
            )
            .expression_attribute_values(
                ":sk_prefix",
                AttributeValue::S("oauth_client#".to_owned()),
            )
            .send()
            .await?
            .items
            .unwrap_or_default();

        Ok(items
            .into_iter()
            .filter_map(|item| match serde_dynamo::from_item::<_, OAuthClientRecord>(item) {
                Ok(record) => Some(record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<OAuthClientRecord>(), "Failed deserializing OAuthClientRecord from query.");
                    None
                }
            })
            .collect())
    }

    async fn update_client_record(
        &self,
        client_id: &OAuthClientId,
        update: OAuthClientRecordUpdate,
    ) -> Result<Option<OAuthClientRecord>, SdkError<UpdateItemError>> {
        let update_expr = update.into_update_expr()?;

        self.client
            .update_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(client_record::mk_pk().to_owned()))
            .key("sk", AttributeValue::S(client_record::mk_sk(client_id)))
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
                            error!(clientId = %client_id, error = %err, type = %std::any::type_name::<OAuthClientRecord>(), "Failed deserializing OAuthClientRecord.");
                            None
                        }
                    })
            })
    }

    async fn delete_client_record(
        &self,
        client_id: &OAuthClientId,
    ) -> Result<DeleteItemOutput, SdkError<DeleteItemError>> {
        self.client
            .delete_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(client_record::mk_pk().to_owned()))
            .key("sk", AttributeValue::S(client_record::mk_sk(client_id)))
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
