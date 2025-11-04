use crate::dynamodb::shop_record::{ShopRecord, mk_pk, mk_pk_as_shop_host, mk_pk_as_shop_id};
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
    shop_id::{ShopId, ShopIdentifier},
};
use std::collections::HashMap;
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
        shop_identifiers: &Batch<ShopIdentifier, 100>,
    ) -> Result<
        BatchGetItemResult<ShopRecord, ShopIdentifier>,
        SdkError<BatchGetItemError, HttpResponse>,
    >;
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
            .key("pk", AttributeValue::S(mk_pk_as_shop_host(shop_url).ok_or(SdkError::construction_failure("failed constructing partition-key because shops url has no valid host"))?))
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
        shop_identifiers: &Batch<ShopIdentifier, 100>,
    ) -> Result<
        BatchGetItemResult<ShopRecord, ShopIdentifier>,
        SdkError<BatchGetItemError, HttpResponse>,
    > {
        let mut failed = Vec::new();
        let keys = shop_identifiers
            .iter()
            .filter_map(|shop_identifier| {
                let mut columns = HashMap::with_capacity(2);
                match mk_pk(shop_identifier) {
                    Some(pk) => {
                        columns.insert("pk".to_owned(), AttributeValue::S(pk));
                        columns.insert(
                            "sk".to_owned(),
                            AttributeValue::S("shop#details".to_owned()),
                        );
                        Some(columns)
                    }
                    None => {
                        error!(
                            shopIdentifier = %shop_identifier,
                            "Failed constructing partition-key because shops url has no valid host."
                        );
                        failed.push(shop_identifier.clone());
                        None
                    }
                }
            })
            .collect();
        let keys_and_attributes = KeysAndAttributes::builder()
            .set_keys(Some(keys))
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

        let mut unprocessed = response
            .unprocessed_keys
            .unwrap_or_default()
            .remove(&self.table)
            .map(|keys_and_attributes| keys_and_attributes.keys)
            .unwrap_or_default()
            .into_iter()
            .filter_map(extract_shop_identifier)
            .collect::<Vec<_>>();

        let batch_result = BatchGetItemResult {
            items: records,
            unprocessed: if unprocessed.is_empty() {
                None
            } else {
                unprocessed.append(&mut failed);
                Some(Batch::try_from(unprocessed).expect(
                    "shouldn't fail creating batch because DynamoDB cannot respond \
                                with more failed ItemKeys than those requested.",
                ))
            },
        };
        Ok(batch_result)
    }
}

fn extract_shop_identifier(attr_map: HashMap<String, AttributeValue>) -> Option<ShopIdentifier> {
    let mut attr_map = attr_map;
    match attr_map.remove("pk") {
        Some(AttributeValue::S(mut key)) => {
            let shop_id_pat = "shop#shop_id#";
            let shop_url_pat = "shop#url#";
            if key.starts_with(shop_id_pat) {
                let shop_id_str = key.split_off(shop_id_pat.len());
                match ShopId::try_from(&shop_id_str) {
                    Ok(shop_id) => Some(ShopIdentifier::ShopId(shop_id)),
                    Err(err) => {
                        error!(error = %err, "Failed parsing extracted ShopId '{shop_id_str}'. This is a bug.");
                        None
                    }
                }
            } else if key.starts_with(shop_url_pat) {
                let shop_url_str = key.split_off(shop_url_pat.len());
                match Url::parse(&shop_url_str) {
                    Ok(url) => Some(ShopIdentifier::ShopUrl(url)),
                    Err(err) => {
                        error!(error = %err, "Failed parsing extracted ShopUrl '{shop_url_str}'. This is a bug.");
                        None
                    }
                }
            } else {
                error!(
                    "Partition-Key for unprocessed key-set started with unexpected prefix '{key}'. This is a bug."
                );
                None
            }
        }
        _ => {
            error!(
                "Failed extracting 'pk' as String from attribute-map in BatchGetItemOutput::unprocessed_keys. This is a bug."
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::dynamodb::repository::extract_shop_identifier;
    use aws_sdk_dynamodb::types::AttributeValue;
    use common::shop_id::{ShopId, ShopIdentifier};
    use std::collections::HashMap;
    use url::Url;

    #[rstest::rstest]
    #[case([].into(), None)]
    #[case([("pk".into(), AttributeValue::S("foo".into()))].into(), None)]
    #[case([("pk".into(), AttributeValue::S("shop#shop_id#bar".into()))].into(), None)]
    #[case([("pk".into(), AttributeValue::S("shop#url#baz".into()))].into(), None)]
    #[case([("pk".into(), AttributeValue::S("shop#shop_id#2a48a17b-cc4f-4489-83cf-f3f215711047".into()))].into(), Some(ShopId::try_from("2a48a17b-cc4f-4489-83cf-f3f215711047").unwrap().into()))]
    #[case([("pk".into(), AttributeValue::S("shop#url#https://foo.bar".into()))].into(), Some(Url::parse("https://foo.bar").unwrap().into()))]
    fn should_extract_shop_identifier(
        #[case] attr_map: HashMap<String, AttributeValue>,
        #[case] expected: Option<ShopIdentifier>,
    ) {
        let actual = extract_shop_identifier(attr_map);

        assert_eq!(expected, actual);
    }
}
