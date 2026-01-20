use crate::dynamodb::product_event_record::ProductEventRecord;
use crate::dynamodb::product_record::{self, ProductRecord};
use crate::dynamodb::product_update_record::ProductRecordUpdate;
use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::SdkError;
use aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError;
use aws_sdk_dynamodb::operation::batch_write_item::{BatchWriteItemError, BatchWriteItemOutput};
use aws_sdk_dynamodb::operation::get_item::GetItemError;
use aws_sdk_dynamodb::operation::query::QueryError;
use aws_sdk_dynamodb::operation::update_item::{UpdateItemError, UpdateItemOutput};
use aws_sdk_dynamodb::types::{AttributeValue, KeysAndAttributes};
use common::batch::Batch;
use common::batch::dynamodb::BatchGetItemResult;
use common::dynamodb_update::DynamoDbUpdate;
use common::product_id::{ProductId, ProductKey};
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::slug_id::SlugId;
use std::collections::HashMap;
use tracing::{error, warn};

#[async_trait]
#[allow(clippy::result_large_err)]
#[mockall::automock]
pub trait ProductDynamoDbRepository {
    async fn put_product_event_records(
        &self,
        product_event_records: Batch<ProductEventRecord, 25>,
    ) -> Result<BatchWriteItemOutput, SdkError<BatchWriteItemError, HttpResponse>>;

    async fn put_product_records(
        &self,
        product_records: Batch<ProductRecord, 25>,
    ) -> Result<BatchWriteItemOutput, SdkError<BatchWriteItemError, HttpResponse>>;

    async fn update_product_record(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
        update: ProductRecordUpdate,
    ) -> Result<UpdateItemOutput, SdkError<UpdateItemError, HttpResponse>>;

    async fn get_product_record(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<Option<ProductRecord>, SdkError<GetItemError, HttpResponse>>;

    async fn query_product_record_and_event_records(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<Option<(ProductRecord, Vec<ProductEventRecord>)>, SdkError<QueryError, HttpResponse>>;

    async fn get_product_records(
        &self,
        product_keys: &Batch<ProductKey, 100>,
    ) -> Result<
        BatchGetItemResult<ProductRecord, ProductKey>,
        SdkError<BatchGetItemError, HttpResponse>,
    >;

    async fn exist_product_records(
        &self,
        product_keys: &Batch<ProductKey, 100>,
    ) -> Result<BatchGetItemResult<ProductKey, ProductKey>, SdkError<BatchGetItemError, HttpResponse>>;

    async fn get_product_id(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<Option<ProductId>, SdkError<GetItemError, HttpResponse>>;

    async fn query_product_key(
        &self,
        shop_slug_id: &SlugId<0>,
        product_slug_id: &SlugId<6>,
    ) -> Result<Option<ProductKey>, SdkError<QueryError, HttpResponse>>;
}

#[derive(Debug, Clone)]
pub struct ProductDynamoDbRepositoryImpl<'a> {
    client: &'a Client,
    table: String,
}

impl<'a> ProductDynamoDbRepositoryImpl<'a> {
    pub fn new(client: &'a Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }
}

#[async_trait]
impl<'a> ProductDynamoDbRepository for ProductDynamoDbRepositoryImpl<'a> {
    async fn put_product_event_records(
        &self,
        product_event_records: Batch<ProductEventRecord, 25>,
    ) -> Result<BatchWriteItemOutput, SdkError<BatchWriteItemError, HttpResponse>> {
        self.client
            .batch_write_item()
            .set_request_items(Some(HashMap::from([(
                self.table.clone(),
                product_event_records.into_dynamodb_write_requests(),
            )])))
            .send()
            .await
    }

    async fn put_product_records(
        &self,
        product_records: Batch<ProductRecord, 25>,
    ) -> Result<BatchWriteItemOutput, SdkError<BatchWriteItemError, HttpResponse>> {
        self.client
            .batch_write_item()
            .set_request_items(Some(HashMap::from([(
                self.table.clone(),
                product_records.into_dynamodb_write_requests(),
            )])))
            .send()
            .await
    }

    async fn update_product_record(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
        product_update_record: ProductRecordUpdate,
    ) -> Result<UpdateItemOutput, SdkError<UpdateItemError, HttpResponse>> {
        let pk = product_record::mk_pk(shop_id, shops_product_id);
        let sk = product_record::mk_sk().to_owned();
        let update_expr = product_update_record.into_update_expr()?;

        self.client
            .update_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(pk))
            .key("sk", AttributeValue::S(sk))
            .update_expression(update_expr.update_expr)
            .set_expression_attribute_names(Some(update_expr.expr_attr_names))
            .set_expression_attribute_values(Some(update_expr.expr_attr_values))
            .send()
            .await
    }

    async fn get_product_record(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<Option<ProductRecord>, SdkError<GetItemError, HttpResponse>> {
        let rec = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(product_record::mk_pk(shop_id, shops_product_id)))
            .key("sk", AttributeValue::S(product_record::mk_sk().to_owned()))
            .send()
            .await?
            .item
            .map(serde_dynamo::from_item::<_, ProductRecord>)
            .and_then(|product_record_res| match product_record_res {
                Ok(product_record) => Some(product_record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<ProductRecord>(), "Failed deserializing ProductRecord.");
                    None
                }
            });

        Ok(rec)
    }

    async fn query_product_record_and_event_records(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<Option<(ProductRecord, Vec<ProductEventRecord>)>, SdkError<QueryError, HttpResponse>>
    {
        let composite = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("#pk = :pk_val AND begins_with(#sk, :sk_prefix)")
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#sk", "sk")
            .expression_attribute_values(
                ":pk_val",
                AttributeValue::S(product_record::mk_pk(shop_id, shops_product_id)),
            )
            .expression_attribute_values(":sk_prefix", AttributeValue::S("product#".to_string()))
            .scan_index_forward(true)
            .into_paginator()
            .send()
            .try_collect()
            .await?
            .into_iter()
            .flat_map(|qo| qo.items.unwrap_or_default())
            .fold((None, None), |(materialized, events), record| match record
                .get("sk")
                .map(AttributeValue::as_s)
                .and_then(Result::ok)
                .map(String::as_str)
            {
                Some("product#materialized") => {
                    match serde_dynamo::from_item::<_, ProductRecord>(record) {
                        Ok(materialized) => (Some(materialized), events),
                        Err(err) => {
                            error!(
                                error = %err,
                                type = %std::any::type_name::<ProductRecord>(),
                                "Failed deserializing ProductRecord."
                            );
                            (materialized, events)
                        },
                    }
                },
                Some(sk) if sk.starts_with("product#event#") => {
                    match serde_dynamo::from_item::<_, ProductEventRecord>(record) {
                        Ok(event) => {
                            let mut events: Vec<ProductEventRecord> = events.unwrap_or_default();
                            events.push(event);
                            (materialized, Some(events))
                        },
                        Err(err) => {
                            error!(
                                error = %err,
                                type = %std::any::type_name::<ProductEventRecord>(),
                                "Failed deserializing ProductEventRecord."
                            );
                            (materialized, events)
                        },
                    }
                },
                Some(unknown_sk) => {
                    error!(
                        payload = ?record,
                        "Attempted to deserialize but record in product-partition contains unknown value '{unknown_sk}' for field 'sk'. Skipping record."
                    );
                    (materialized, events)
                },
                None => {
                    error!(
                        payload = ?record,
                        "Attempted to deserialize record in product-partition but no String-Field 'sk' exists. Skipping record."
                    );
                    (materialized, events)
                }
            });

        match composite {
            (None, None) => Ok(None),
            (None, Some(_)) => {
                warn!("Materialized ProductRecord does not exist.");
                Ok(None)
            }
            (Some(materialized), None) => Ok(Some((materialized, vec![]))),
            (Some(materialized), Some(events)) => Ok(Some((materialized, events))),
        }
    }

    async fn get_product_records(
        &self,
        product_keys: &Batch<ProductKey, 100>,
    ) -> Result<
        BatchGetItemResult<ProductRecord, ProductKey>,
        SdkError<BatchGetItemError, HttpResponse>,
    > {
        let keys = product_keys
            .iter()
            .map(|product_key| {
                let mut columns = HashMap::with_capacity(2);
                columns.insert(
                    "pk".to_owned(),
                    AttributeValue::S(product_record::mk_pk(
                        &product_key.shop_id,
                        &product_key.shops_product_id,
                    )),
                );
                columns.insert(
                    "sk".to_owned(),
                    AttributeValue::S(product_record::mk_sk().to_owned()),
                );
                columns
            })
            .collect();
        let keys_and_attributes = KeysAndAttributes::builder()
            .set_keys(Some(keys))
            .build()
            .expect("shouldn't fail because we previously set the only required field 'keys'.");
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
            .map(serde_dynamo::from_item::<_, ProductRecord>)
            .filter_map(|result| match result {
                Ok(event) => Some(event),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<ProductRecord>(), "Failed deserializing ProductRecord.");
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
            .filter_map(|attr_map| match extract_product_key(attr_map) {
                Ok(key) => Some(key),
                Err(err) => {
                    error!(
                        error = err,
                        "Failed extracting ProductKey from BatchGetItemOutput::unprocessed_keys."
                    );
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

    async fn exist_product_records(
        &self,
        product_keys: &Batch<ProductKey, 100>,
    ) -> Result<BatchGetItemResult<ProductKey, ProductKey>, SdkError<BatchGetItemError, HttpResponse>>
    {
        let keys = product_keys
            .iter()
            .map(|product_key| {
                let mut columns = HashMap::with_capacity(2);
                columns.insert(
                    "pk".to_owned(),
                    AttributeValue::S(product_record::mk_pk(
                        &product_key.shop_id,
                        &product_key.shops_product_id,
                    )),
                );
                columns.insert(
                    "sk".to_owned(),
                    AttributeValue::S(product_record::mk_sk().to_owned()),
                );
                columns
            })
            .collect();
        let keys_and_attributes = KeysAndAttributes::builder()
            .set_keys(Some(keys))
            .projection_expression("pk")
            .build()
            .expect("shouldn't fail because we previously set the only required field 'keys'.");
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
            .map(extract_product_key)
            .filter_map(|result| match result {
                Ok(event) => Some(event),
                Err(err) => {
                    error!(error = %err, "Failed extracting ProductKey.");
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
            .filter_map(|attr_map| match extract_product_key(attr_map) {
                Ok(key) => Some(key),
                Err(err) => {
                    error!(
                        error = err,
                        "Failed extracting ProductKey from BatchGetItemOutput::unprocessed_keys."
                    );
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

    async fn get_product_id(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<Option<ProductId>, SdkError<GetItemError, HttpResponse>> {
        let product_id = self
            .client
            .get_item()
            .table_name(&self.table)
            .key(
                "pk",
                AttributeValue::S(product_record::mk_pk(shop_id, shops_product_id)),
            )
            .key("sk", AttributeValue::S(product_record::mk_sk().to_owned()))
            .projection_expression("product_id")
            .send()
            .await?
            .item
            .map(extract_product_id)
            .transpose()
            .map_err(SdkError::construction_failure)?; // not ideal semantically - but eases types

        Ok(product_id)
    }

    async fn query_product_key(
        &self,
        shop_slug_id: &SlugId<0>,
        product_slug_id: &SlugId<6>,
    ) -> Result<Option<ProductKey>, SdkError<QueryError, HttpResponse>> {
        self.client
            .query()
            .table_name(&self.table)
            .index_name("gsi2")
            .key_condition_expression("#gsi2_pk = :gsi2_pk_val AND #gsi2_sk = :gsi2_sk_val")
            .expression_attribute_names("#gsi2_pk", "gsi2_pk")
            .expression_attribute_names("#gsi2_sk", "gsi2_sk")
            .expression_attribute_values(
                ":gsi2_pk_val",
                AttributeValue::S(product_record::mk_gsi2_pk(shop_slug_id, product_slug_id)),
            )
            .expression_attribute_values(
                ":gsi2_sk_val",
                AttributeValue::S(product_record::mk_gsi2_sk().to_owned()),
            )
            .limit(1)
            .send()
            .await?
            .items
            .unwrap_or_default()
            .into_iter()
            .next()
            .map(|mut attr_map| {
                attr_map
                    .remove("pk")
                    .ok_or(SdkError::construction_failure(format!(
                    "DynamoDB Reponse attribute-map did not contain field 'pk' when querying gsi2
                        for shop_slug_id '{shop_slug_id}' and product_slug_id '{product_slug_id}'"
                )))
            })
            .transpose()?
            .map(|v| v.as_s().expect("shouldn't fail extracting 'pk' as String because PKs are always Strings for us").clone())
            .map(|s| ProductKey::try_from(s.as_str().trim_start_matches("product#")))
            .transpose()
            .map_err(SdkError::construction_failure)
    }
}

pub fn extract_product_key(map: HashMap<String, AttributeValue>) -> Result<ProductKey, String> {
    let mut map = map;

    // ugly af but much more efficient due to slices than using iterators in functional-style here
    if let Some(pk_attr) = map.remove("pk") {
        let pk_res = pk_attr.as_s();
        if let Ok(pk) = pk_res {
            if let Some((shop_id, shops_product_id)) = pk
                .trim_start_matches("product#shop_id#")
                .split_once("#shops_product_id#")
            {
                Ok(ProductKey {
                    shop_id: shop_id.try_into().unwrap(),
                    shops_product_id: shops_product_id.into(),
                })
            } else {
                Err(format!("Parsing pk '{pk}' failed."))
            }
        } else {
            Err(format!("Extracted value for pk '{pk_attr:?}' failed."))
        }
    } else {
        Err(format!(
            "AttributeValue-Map does not contain key pk: '{map:?}'."
        ))
    }
}

fn extract_product_id(
    map: HashMap<String, AttributeValue>,
) -> Result<ProductId, Box<dyn std::error::Error + Send + Sync>> {
    let mut map = map;

    // ugly af but much more efficient due to slices than using iterators in functional-style here
    if let Some(product_id_attr) = map.remove("product_id") {
        let product_id_res = product_id_attr.as_s();
        if let Ok(product_id_str) = product_id_res {
            Ok(ProductId::try_from(product_id_str.as_str())?)
        } else {
            Err(format!("Extracted value for product_id '{product_id_attr:?}' failed.").into())
        }
    } else {
        Err(format!("AttributeValue-Map does not contain key product_id: '{map:?}'.").into())
    }
}

#[cfg(test)]
mod tests {
    use rstest;

    use crate::dynamodb::repository::{extract_product_id, extract_product_key};
    use aws_sdk_dynamodb::types::AttributeValue;
    use common::product_id::{ProductId, ProductKey};
    use std::collections::HashMap;

    #[rstest::rstest]
    #[trace]
    #[case::differing("a1caead3-a50d-44a4-b9fb-a15d2397601e", "123456")]
    #[case::containing_separator("a1caead3-a50d-44a4-b9fb-a15d2397601e", "abcdefg#boop")]
    fn should_extract_product_key_from_pk_sk_map_when_pk_exists_and_is_valid_for(
        #[case] shop_id: &str,
        #[case] shops_product_id: &str,
    ) {
        let map = HashMap::from([(
            "pk".to_owned(),
            AttributeValue::S(format!(
                "product#shop_id#{shop_id}#shops_product_id#{shops_product_id}"
            )),
        )]);
        let expected = ProductKey {
            shop_id: shop_id.try_into().unwrap(),
            shops_product_id: shops_product_id.into(),
        };

        let actual = extract_product_key(map);

        assert!(actual.is_ok());
        assert_eq!(expected, actual.unwrap());
    }

    #[rstest::rstest]
    #[trace]
    #[case("a1caead3-a50d-44a4-b9fb-a15d2397601e")]
    #[case("6e3f0c71-8af4-4897-ba75-4a64792c07a6")]
    #[case("f5e5aa19-7d97-4972-af6f-426f1ab8bb8f")]
    fn should_extract_product_id_from_map_when_product_id_exists_and_is_valid_for(
        #[case] product_id_str: &str,
    ) {
        let map = HashMap::from([(
            "product_id".to_owned(),
            AttributeValue::S(product_id_str.to_owned()),
        )]);
        let expected = ProductId::try_from(product_id_str).unwrap();

        let actual = extract_product_id(map);

        assert!(actual.is_ok());
        assert_eq!(expected, actual.unwrap());
    }
}
