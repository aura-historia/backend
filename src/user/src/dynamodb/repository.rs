use crate::dynamodb::{
    user_record::{UserRecord, mk_gsi1_pk, mk_gsi1_sk, mk_pk, mk_sk},
    user_record_update::UserRecordUpdate,
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
use common::{
    dynamodb_update::DynamoDbUpdate, stripe_customer_id::StripeCustomerId, user_id::UserId,
};
use tracing::error;

#[async_trait::async_trait]
#[mockall::automock]
pub trait UserDynamoDbRepository {
    async fn get_user_record(
        &self,
        user_id: &UserId,
    ) -> Result<Option<UserRecord>, SdkError<GetItemError>>;

    async fn find_user_record_by_stripe_customer_id(
        &self,
        stripe_customer_id: &StripeCustomerId,
    ) -> Result<Option<UserRecord>, SdkError<QueryError>>;

    async fn put_user_record(
        &self,
        user_record: UserRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError>>;

    async fn update_user_record(
        &self,
        user_id: &UserId,
        user_record_update: UserRecordUpdate,
    ) -> Result<Option<UserRecord>, SdkError<UpdateItemError>>;

    /// Sets the user's tier and **removes** the Stripe-related attributes
    /// (`stripe_customer_id`, `gsi1_pk`, `gsi1_sk`) atomically. Used when a
    /// Stripe subscription is deleted to fully detach the user from their
    /// previous Stripe customer-id.
    async fn clear_stripe_subscription_and_set_tier(
        &self,
        user_id: &UserId,
        tier: crate::dynamodb::tier_record::UserTierRecord,
    ) -> Result<Option<UserRecord>, SdkError<UpdateItemError>>;

    async fn delete_user_record(
        &self,
        user_id: &UserId,
    ) -> Result<DeleteItemOutput, SdkError<DeleteItemError>>;
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

    async fn find_user_record_by_stripe_customer_id(
        &self,
        stripe_customer_id: &StripeCustomerId,
    ) -> Result<Option<UserRecord>, SdkError<QueryError>> {
        let items = self
            .client
            .query()
            .table_name(&self.table)
            .index_name("gsi1")
            .key_condition_expression("#gsi1_pk = :gsi1_pk_val AND #gsi1_sk = :gsi1_sk_val")
            .expression_attribute_names("#gsi1_pk", "gsi1_pk")
            .expression_attribute_names("#gsi1_sk", "gsi1_sk")
            .expression_attribute_values(
                ":gsi1_pk_val",
                AttributeValue::S(mk_gsi1_pk(stripe_customer_id)),
            )
            .expression_attribute_values(":gsi1_sk_val", AttributeValue::S(mk_gsi1_sk().to_owned()))
            .limit(1)
            .send()
            .await?
            .items
            .unwrap_or_default();

        let rec = items.into_iter().next().and_then(|item| {
            match serde_dynamo::from_item::<_, UserRecord>(item) {
                Ok(record) => Some(record),
                Err(err) => {
                    error!(
                        error = %err,
                        type = %std::any::type_name::<UserRecord>(),
                        "Failed deserializing UserRecord from gsi1 query."
                    );
                    None
                }
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
                        Ok(user_record) => Some(user_record),
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

    async fn clear_stripe_subscription_and_set_tier(
        &self,
        user_id: &UserId,
        tier: crate::dynamodb::tier_record::UserTierRecord,
    ) -> Result<Option<UserRecord>, SdkError<UpdateItemError>> {
        let tier_av =
            serde_dynamo::to_attribute_value(tier).map_err(SdkError::construction_failure)?;
        let now_av = serde_dynamo::to_attribute_value(time::OffsetDateTime::now_utc())
            .map_err(SdkError::construction_failure)?;

        self.client
            .update_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk().to_owned()))
            .update_expression(
                "SET #tier = :tier, #updated = :updated \
                 REMOVE #stripe_customer_id, #gsi1_pk, #gsi1_sk",
            )
            .expression_attribute_names("#tier", "tier")
            .expression_attribute_names("#updated", "updated")
            .expression_attribute_names("#stripe_customer_id", "stripe_customer_id")
            .expression_attribute_names("#gsi1_pk", "gsi1_pk")
            .expression_attribute_names("#gsi1_sk", "gsi1_sk")
            .expression_attribute_values(":tier", tier_av)
            .expression_attribute_values(":updated", now_av)
            .return_values(ReturnValue::AllNew)
            .send()
            .await
            .map(|output| output.attributes)
            .map(|attr_opt| {
                attr_opt
                    .map(serde_dynamo::from_item)
                    .and_then(|record_res| match record_res {
                        Ok(user_record) => Some(user_record),
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

    async fn delete_user_record(
        &self,
        user_id: &UserId,
    ) -> Result<DeleteItemOutput, SdkError<DeleteItemError>> {
        self.client
            .delete_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk().to_owned()))
            .send()
            .await
    }
}
