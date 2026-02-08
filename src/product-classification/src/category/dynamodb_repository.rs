use crate::category::record::{CategoryRecord, mk_pk, mk_sk};
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
use common::category_key::CategoryId;
use tracing::error;

#[async_trait::async_trait]
#[mockall::automock]
pub trait CategoryDynamoDbRepository {
    async fn put_category_record(
        &self,
        record: CategoryRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError>>;

    async fn get_category_record(
        &self,
        category_id: &CategoryId,
    ) -> Result<Option<CategoryRecord>, SdkError<GetItemError>>;

    async fn query_category_records(&self) -> Result<Vec<CategoryRecord>, SdkError<QueryError>>;
}

#[derive(Debug, Clone)]
pub struct CategoryDynamoDbRepositoryImpl<'a> {
    client: &'a Client,
    table: String,
}

impl<'a> CategoryDynamoDbRepositoryImpl<'a> {
    pub fn new(client: &'a Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }
}

#[async_trait::async_trait]
impl<'a> CategoryDynamoDbRepository for CategoryDynamoDbRepositoryImpl<'a> {
    async fn put_category_record(
        &self,
        record: CategoryRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError>> {
        let payload = serde_dynamo::to_item(record).map_err(SdkError::construction_failure)?;
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(payload))
            .send()
            .await
    }

    async fn get_category_record(
        &self,
        category_id: &CategoryId,
    ) -> Result<Option<CategoryRecord>, SdkError<GetItemError>> {
        let rec = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk().to_owned()))
            .key("sk", AttributeValue::S(mk_sk(category_id)))
            .send()
            .await?
            .item
            .map(serde_dynamo::from_item::<_, CategoryRecord>)
            .and_then(|category_record_res| match category_record_res {
                Ok(category_record) => Some(category_record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<CategoryRecord>(), "Failed deserializing.");
                    None
                }
            });

        Ok(rec)
    }

    async fn query_category_records(&self) -> Result<Vec<CategoryRecord>, SdkError<QueryError>> {
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
            .expression_attribute_values(":sk_prefix", AttributeValue::S("category#".to_string()))
            .send()
            .await?
            .items
            .unwrap_or_default()
            .into_iter()
            .map(serde_dynamo::from_item::<_, CategoryRecord>)
            .filter_map(|res| match res {
                Ok(record) => Some(record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<CategoryRecord>(), "Failed deserializing.");
                    None
                }
            })
            .collect();

        Ok(records)
    }
}
