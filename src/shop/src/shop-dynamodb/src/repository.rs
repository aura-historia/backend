use std::collections::HashMap;

use crate::shop_record::{ShopRecord, mk_pk_as_shop_id, mk_pk_as_shop_url};
use aws_sdk_dynamodb::{
    Client,
    config::http::HttpResponse,
    error::SdkError,
    operation::{
        batch_get_item::BatchGetItemError,
        get_item::GetItemError,
        put_item::{PutItemError, PutItemOutput},
        transact_write_items::{TransactWriteItemsError, TransactWriteItemsOutput},
    },
    types::{AttributeValue, KeysAndAttributes, Put, TransactWriteItem},
};
use common::{
    batch::{Batch, dynamodb::BatchGetItemResult},
    shop_id::ShopId,
};
use tracing::error;
use url::Url;

#[async_trait::async_trait]
#[mockall::automock]
pub trait ShopDynamoDbRepository {
    async fn put_shop_record(
        &self,
        record: ShopRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError, HttpResponse>>;

    async fn put_shop_records_transact(
        &self,
        records: Vec<ShopRecord>,
    ) -> Result<TransactWriteItemsOutput, SdkError<TransactWriteItemsError, HttpResponse>>;

    async fn get_shop_record_by_id(
        &self,
        shop_id: &ShopId,
    ) -> Result<Option<ShopRecord>, SdkError<GetItemError, HttpResponse>>;

    async fn get_shop_record_by_url(
        &self,
        shop_url: &Url,
    ) -> Result<Option<ShopRecord>, SdkError<GetItemError, HttpResponse>>;

    async fn get_shop_records(
        &self,
        item_keys: &Batch<String, 100>,
    ) -> Result<BatchGetItemResult<ShopRecord, String>, SdkError<BatchGetItemError, HttpResponse>>;
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

    async fn put_shop_records_transact(
        &self,
        records: Vec<ShopRecord>,
    ) -> Result<TransactWriteItemsOutput, SdkError<TransactWriteItemsError, HttpResponse>> {
        let payloads = records
            .into_iter()
            .map(|record| {
                let shop_id = record.shop_id;
                serde_dynamo::to_item(record).map(|item| (shop_id, item))
            })
            .collect::<Result<Vec<_>, serde_dynamo::Error>>()
            .map_err(SdkError::construction_failure)?
            .into_iter()
            .map(|(shop_id, item)| {
                TransactWriteItem::builder()
                    .put(
                        Put::builder()
                            .table_name(&self.table)
                            .set_item(Some(item))
                            .condition_expression("(attribute_not_exists(#pk) AND attribute_not_exists(#sk)) OR #shop_id = :shop_id")
                            .expression_attribute_names("#pk", "pk")
                            .expression_attribute_names("#sk", "sk")
                            .expression_attribute_names("#shop_id", "shop_id")
                            .expression_attribute_values(":shop_id", AttributeValue::S(shop_id.to_string()))
                            .build()
                            .expect("shouldn't fail because 'table_name' and 'item' have been set"),
                    )
                    .build()
            })
            .collect();
        self.client
            .transact_write_items()
            .set_transact_items(Some(payloads))
            .send()
            .await
    }

    async fn get_shop_record_by_id(
        &self,
        shop_id: &ShopId,
    ) -> Result<Option<ShopRecord>, SdkError<GetItemError, HttpResponse>> {
        let rec = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk_as_shop_id(shop_id)))
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

    async fn get_shop_record_by_url(
        &self,
        shop_url: &Url,
    ) -> Result<Option<ShopRecord>, SdkError<GetItemError, HttpResponse>> {
        let rec = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk_as_shop_url(shop_url)))
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

    async fn get_shop_records(
        &self,
        pks: &Batch<String, 100>,
    ) -> Result<BatchGetItemResult<ShopRecord, String>, SdkError<BatchGetItemError, HttpResponse>>
    {
        let pks = pks
            .iter()
            .map(|pk| {
                let mut columns = HashMap::with_capacity(2);
                columns.insert("pk".to_owned(), AttributeValue::S(pk.to_string()));
                columns.insert(
                    "sk".to_owned(),
                    AttributeValue::S("shop#details".to_owned()),
                );
                columns
            })
            .collect();
        let keys_and_attributes = KeysAndAttributes::builder()
            .set_keys(Some(pks))
            .build()
            .expect("shouldn't fail because we previously set the only required field 'keys'");
        let request_items = Some(HashMap::from([(self.table.clone(), keys_and_attributes)]));
        let response = self
            .client
            .batch_get_item()
            .set_request_items(request_items)
            .send()
            .await?;

        let records = response
            .responses
            .unwrap_or_default()
            .remove(&self.table)
            .unwrap_or_default()
            .into_iter()
            .map(serde_dynamo::from_item::<_, ShopRecord>)
            .filter_map(|result| match result {
                Ok(event) => Some(event),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<ShopRecord>(), "Failed deserializing.");
                    None
                }
            })
            .collect::<Vec<_>>();

        let unprocessed = response
            .unprocessed_keys
            .unwrap_or_default()
            .remove(&self.table)
            .map(|keys_and_attributes| keys_and_attributes.keys)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|mut attr_map| match attr_map.remove("pk") {
                Some(AttributeValue::S(key)) => Some(key),
                _ => {
                    error!("Failed extracting 'pk' as String from attribute-map in BatchGetItemOutput::unprocessed_keys. This is a bug.");
                    None
                }
            })
            .collect::<Vec<_>>();

        let batch_result = BatchGetItemResult {
            items: records,
            unprocessed: if unprocessed.is_empty() {
                None
            } else {
                Some(Batch::try_from(unprocessed).expect(
                    "shouldn't fail creating batch because DynamoDB cannot respond \
                                with more failed ItemKeys than those requested.",
                ))
            },
        };
        Ok(batch_result)
    }
}
