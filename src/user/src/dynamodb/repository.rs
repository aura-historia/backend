use crate::dynamodb::{
    user_record::{UserRecord, mk_pk, mk_sk},
    user_record_update::UserRecordUpdate,
};
use aws_sdk_dynamodb::{
    Client,
    error::SdkError,
    operation::{
        get_item::GetItemError,
        put_item::{PutItemError, PutItemOutput},
        update_item::UpdateItemError,
    },
    types::{AttributeValue, ReturnValue},
};
use common::{dynamodb_update::DynamoDbUpdate, user_id::UserId};
use tracing::error;

#[async_trait::async_trait]
#[mockall::automock]
pub trait UserDynamoDbRepository {
    async fn get_user_record(
        &self,
        user_id: &UserId,
    ) -> Result<Option<UserRecord>, SdkError<GetItemError>>;

    async fn put_user_record(
        &self,
        user_record: UserRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError>>;

    async fn update_user_record(
        &self,
        user_id: &UserId,
        user_record_update: UserRecordUpdate,
    ) -> Result<Option<UserRecord>, SdkError<UpdateItemError>>;
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
    ) -> Result<Option<UserRecord>, SdkError<GetItemError>> {
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
                Ok(product_record) => Some(product_record),
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
    ) -> Result<PutItemOutput, SdkError<PutItemError>> {
        let payload = serde_dynamo::to_item(user_record).map_err(SdkError::construction_failure)?;
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(payload))
            .send()
            .await
    }

    async fn update_user_record(
        &self,
        user_id: &UserId,
        user_record_update: UserRecordUpdate,
    ) -> Result<Option<UserRecord>, SdkError<UpdateItemError>> {
        let update_expr = user_record_update.into_update_expr()?;

        self.client
            .update_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk().to_owned()))
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
                        Ok(search_filter_record) => Some(search_filter_record),
                        Err(err) => {
                            error!(
                                userId = %user_id,
                                error = %err,
                                type = %std::any::type_name::<UserRecord>(),
                                "Failed deserializing UserRecord."
                            );
                            None
                        }
                    })
            })
    }
}
