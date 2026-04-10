use crate::core::partner_shop_application_id::PartnerShopApplicationId;
use crate::dynamodb::{
    partner_shop_application_record::{
        PartnerShopApplicationRecord, mk_gsi1_pk, mk_gsi1_sk, mk_pk, mk_sk,
    },
    partner_shop_application_record_update::PartnerShopApplicationRecordUpdate,
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
use common::{dynamodb_update::DynamoDbUpdate, user_id::UserId};
use tracing::error;

#[async_trait::async_trait]
#[mockall::automock]
pub trait PartnerShopApplicationDynamoDbRepository {
    async fn put_partner_shop_application_record(
        &self,
        record: PartnerShopApplicationRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError>>;

    async fn get_partner_shop_application_record(
        &self,
        user_id: &UserId,
        id: &PartnerShopApplicationId,
    ) -> Result<Option<PartnerShopApplicationRecord>, SdkError<GetItemError>>;

    async fn update_partner_shop_application_record(
        &self,
        user_id: &UserId,
        id: &PartnerShopApplicationId,
        update: PartnerShopApplicationRecordUpdate,
    ) -> Result<Option<PartnerShopApplicationRecord>, SdkError<UpdateItemError>>;

    async fn delete_partner_shop_application_record(
        &self,
        user_id: &UserId,
        id: &PartnerShopApplicationId,
    ) -> Result<DeleteItemOutput, SdkError<DeleteItemError>>;

    async fn query_all_partner_shop_application_records(
        &self,
    ) -> Result<Vec<PartnerShopApplicationRecord>, SdkError<QueryError>>;

    async fn query_partner_shop_application_record_by_id(
        &self,
        id: &PartnerShopApplicationId,
    ) -> Result<Option<PartnerShopApplicationRecord>, SdkError<QueryError>>;

    async fn query_all_partner_shop_application_records_by_user(
        &self,
        user_id: &UserId,
    ) -> Result<Vec<PartnerShopApplicationRecord>, SdkError<QueryError>>;
}

#[derive(Debug, Clone)]
pub struct PartnerShopApplicationDynamoDbRepositoryImpl<'a> {
    client: &'a Client,
    table: String,
}

impl<'a> PartnerShopApplicationDynamoDbRepositoryImpl<'a> {
    pub fn new(client: &'a Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }
}

#[async_trait::async_trait]
impl<'a> PartnerShopApplicationDynamoDbRepository
    for PartnerShopApplicationDynamoDbRepositoryImpl<'a>
{
    async fn put_partner_shop_application_record(
        &self,
        record: PartnerShopApplicationRecord,
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

    async fn get_partner_shop_application_record(
        &self,
        user_id: &UserId,
        id: &PartnerShopApplicationId,
    ) -> Result<Option<PartnerShopApplicationRecord>, SdkError<GetItemError>> {
        let record = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk(id)))
            .send()
            .await?
            .item
            .map(serde_dynamo::from_item::<_, PartnerShopApplicationRecord>)
            .and_then(|res| match res {
                Ok(record) => Some(record),
                Err(err) => {
                    error!(
                        userId = %user_id,
                        partnerShopApplicationId = %id,
                        error = %err,
                        r#type = %std::any::type_name::<PartnerShopApplicationRecord>(),
                        "Failed deserializing PartnerShopApplicationRecord."
                    );
                    None
                }
            });

        Ok(record)
    }

    async fn update_partner_shop_application_record(
        &self,
        user_id: &UserId,
        id: &PartnerShopApplicationId,
        update: PartnerShopApplicationRecordUpdate,
    ) -> Result<Option<PartnerShopApplicationRecord>, SdkError<UpdateItemError>> {
        let update_expr = update.into_update_expr()?;

        self.client
            .update_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk(id)))
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
                            error!(
                                userId = %user_id,
                                partnerShopApplicationId = %id,
                                error = %err,
                                r#type = %std::any::type_name::<PartnerShopApplicationRecord>(),
                                "Failed deserializing PartnerShopApplicationRecord."
                            );
                            None
                        }
                    })
            })
    }

    async fn delete_partner_shop_application_record(
        &self,
        user_id: &UserId,
        id: &PartnerShopApplicationId,
    ) -> Result<DeleteItemOutput, SdkError<DeleteItemError>> {
        self.client
            .delete_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(user_id)))
            .key("sk", AttributeValue::S(mk_sk(id)))
            .send()
            .await
    }

    async fn query_all_partner_shop_application_records(
        &self,
    ) -> Result<Vec<PartnerShopApplicationRecord>, SdkError<QueryError>> {
        let records = self
            .client
            .query()
            .table_name(&self.table)
            .index_name("gsi1")
            .key_condition_expression("#gsi1_pk = :gsi1_pk_val")
            .expression_attribute_names("#gsi1_pk", "gsi1_pk")
            .expression_attribute_values(
                ":gsi1_pk_val",
                AttributeValue::S(mk_gsi1_pk().to_string()),
            )
            .into_paginator()
            .send()
            .try_collect()
            .await?
            .into_iter()
            .flat_map(|query_output| query_output.items.unwrap_or_default())
            .filter_map(|item| {
                match serde_dynamo::from_item::<_, PartnerShopApplicationRecord>(item) {
                    Ok(record) => Some(record),
                    Err(err) => {
                        error!(
                            error = %err,
                            r#type = %std::any::type_name::<PartnerShopApplicationRecord>(),
                            "Failed deserializing PartnerShopApplicationRecord."
                        );
                        None
                    }
                }
            })
            .collect();

        Ok(records)
    }

    async fn query_partner_shop_application_record_by_id(
        &self,
        id: &PartnerShopApplicationId,
    ) -> Result<Option<PartnerShopApplicationRecord>, SdkError<QueryError>> {
        let records: Vec<PartnerShopApplicationRecord> = self
            .client
            .query()
            .table_name(&self.table)
            .index_name("gsi1")
            .key_condition_expression("#gsi1_pk = :gsi1_pk_val AND #gsi1_sk = :gsi1_sk_val")
            .expression_attribute_names("#gsi1_pk", "gsi1_pk")
            .expression_attribute_names("#gsi1_sk", "gsi1_sk")
            .expression_attribute_values(
                ":gsi1_pk_val",
                AttributeValue::S(mk_gsi1_pk().to_string()),
            )
            .expression_attribute_values(":gsi1_sk_val", AttributeValue::S(mk_gsi1_sk(id)))
            .send()
            .await?
            .items
            .unwrap_or_default()
            .into_iter()
            .filter_map(|item| {
                match serde_dynamo::from_item::<_, PartnerShopApplicationRecord>(item) {
                    Ok(record) => Some(record),
                    Err(err) => {
                        error!(
                            partnerShopApplicationId = %id,
                            error = %err,
                            r#type = %std::any::type_name::<PartnerShopApplicationRecord>(),
                            "Failed deserializing PartnerShopApplicationRecord."
                        );
                        None
                    }
                }
            })
            .collect();

        Ok(records.into_iter().next())
    }

    async fn query_all_partner_shop_application_records_by_user(
        &self,
        user_id: &UserId,
    ) -> Result<Vec<PartnerShopApplicationRecord>, SdkError<QueryError>> {
        let records = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("#pk = :pk_val AND begins_with(#sk, :sk_prefix)")
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#sk", "sk")
            .expression_attribute_values(":pk_val", AttributeValue::S(mk_pk(user_id)))
            .expression_attribute_values(
                ":sk_prefix",
                AttributeValue::S("partner_shop_application_id#".to_string()),
            )
            .into_paginator()
            .send()
            .try_collect()
            .await?
            .into_iter()
            .flat_map(|query_output| query_output.items.unwrap_or_default())
            .filter_map(|item| {
                match serde_dynamo::from_item::<_, PartnerShopApplicationRecord>(item) {
                    Ok(record) => Some(record),
                    Err(err) => {
                        error!(
                            userId = %user_id,
                            error = %err,
                            r#type = %std::any::type_name::<PartnerShopApplicationRecord>(),
                            "Failed deserializing PartnerShopApplicationRecord."
                        );
                        None
                    }
                }
            })
            .collect();

        Ok(records)
    }
}
