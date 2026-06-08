use crate::dynamodb::{
    raw_shop_name_record::{self, RawShopNameRecord},
    shop_record::{self, ShopRecord, mk_pk, mk_sk},
    shop_record_update::ShopRecordUpdate,
};
use aws_sdk_dynamodb::{
    Client,
    config::http::HttpResponse,
    error::SdkError,
    operation::{
        batch_get_item::BatchGetItemError,
        get_item::GetItemError,
        put_item::{PutItemError, PutItemOutput},
        query::QueryError,
        update_item::UpdateItemError,
    },
    types::{AttributeValue, KeysAndAttributes, ReturnValue},
};
use common::{
    batch::{Batch, dynamodb::BatchGetItemResult},
    domain::Domain,
    dynamodb_update::DynamoDbUpdate,
    shop_id::ShopId,
    shop_name::ShopName,
    shop_slug_id::ShopSlugId,
};
use std::collections::HashMap;
use tracing::error;

#[async_trait::async_trait]
#[mockall::automock]
pub trait ShopDynamoDbRepository {
    async fn put_shop_record(
        &self,
        record: ShopRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError, HttpResponse>>;

    async fn update_shop_record(
        &self,
        shop_id: &ShopId,
        update: ShopRecordUpdate,
    ) -> Result<Option<ShopRecord>, SdkError<UpdateItemError>>;

    async fn get_shop_record(
        &self,
        shop_id: &ShopId,
    ) -> Result<Option<ShopRecord>, SdkError<GetItemError, HttpResponse>>;

    async fn query_shop_id(
        &self,
        shop_slug_id: &ShopSlugId,
    ) -> Result<Option<ShopId>, SdkError<QueryError, HttpResponse>>;

    async fn get_shop_records(
        &self,
        shop_ids: &Batch<ShopId, 100>,
    ) -> Result<BatchGetItemResult<ShopRecord, ShopId>, SdkError<BatchGetItemError, HttpResponse>>;

    async fn get_raw_shop_name_record(
        &self,
        raw_shop_name: &ShopName,
    ) -> Result<Option<RawShopNameRecord>, SdkError<GetItemError>>;

    async fn put_raw_shop_name_record(
        &self,
        record: RawShopNameRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError, HttpResponse>>;

    async fn query_shop_by_shopify_domain(
        &self,
        shopify_domain: &Domain,
    ) -> Result<Option<ShopRecord>, SdkError<QueryError, HttpResponse>>;
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

    async fn update_shop_record(
        &self,
        shop_id: &ShopId,
        update: ShopRecordUpdate,
    ) -> Result<Option<ShopRecord>, SdkError<UpdateItemError>> {
        let update_expr = update.into_update_expr()?;

        self.client
            .update_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(shop_id)))
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
                                shopId = %shop_id,
                                error = %err,
                                type = %std::any::type_name::<ShopRecord>(),
                                "Failed deserializing ShopRecord."
                            );
                            None
                        }
                    })
            })
    }

    async fn get_shop_record(
        &self,
        shop_id: &ShopId,
    ) -> Result<Option<ShopRecord>, SdkError<GetItemError, HttpResponse>> {
        let rec = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(mk_pk(shop_id)))
            .key("sk", AttributeValue::S("shop#details".to_owned()))
            .send()
            .await?
            .item
            .map(serde_dynamo::from_item::<_, ShopRecord>)
            .and_then(|shop_record_res| match shop_record_res {
                Ok(shop_record) => Some(shop_record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<ShopRecord>(), "Failed deserializing.");
                    None
                }
            });

        Ok(rec)
    }

    async fn query_shop_id(
        &self,
        shop_slug_id: &ShopSlugId,
    ) -> Result<Option<ShopId>, SdkError<QueryError, HttpResponse>> {
        let maybe_item = self
            .client
            .query()
            .table_name(&self.table)
            .index_name("gsi2")
            .key_condition_expression("#gsi2_pk = :gsi2_pk_val AND #gsi2_sk = :gsi2_sk_val")
            .expression_attribute_names("#gsi2_pk", "gsi2_pk")
            .expression_attribute_names("#gsi2_sk", "gsi2_sk")
            .expression_attribute_values(
                ":gsi2_pk_val",
                AttributeValue::S(shop_record::mk_gsi2_pk(shop_slug_id)),
            )
            .expression_attribute_values(
                ":gsi2_sk_val",
                AttributeValue::S(shop_record::mk_gsi2_sk().to_owned()),
            )
            .limit(1)
            .send()
            .await?
            .items
            .unwrap_or_default()
            .into_iter()
            .next();

        let Some(mut attr_map) = maybe_item else {
            return Ok(None);
        };

        let pk_attr = attr_map
            .remove("pk")
            .ok_or_else(|| SdkError::construction_failure(format!(
                "DynamoDB Response attribute-map did not contain field 'pk' when querying gsi2 for shop_slug_id '{shop_slug_id}'"
            )))?;

        let pk_str = pk_attr
            .as_s()
            .expect(
                "shouldn't fail extracting 'pk' as String because PKs are always Strings for us",
            )
            .clone();

        ShopId::try_from(pk_str.trim_start_matches("shop#shop_id#").to_owned())
            .map(Some)
            .map_err(SdkError::construction_failure)
    }

    async fn get_shop_records(
        &self,
        shop_ids: &Batch<ShopId, 100>,
    ) -> Result<BatchGetItemResult<ShopRecord, ShopId>, SdkError<BatchGetItemError, HttpResponse>>
    {
        let mut failed = Vec::new();
        let keys = shop_ids
            .iter()
            .map(|shop_identifier| {
                let mut columns = HashMap::with_capacity(2);
                let pk = mk_pk(shop_identifier);
                columns.insert("pk".to_owned(), AttributeValue::S(pk));
                columns.insert(
                    "sk".to_owned(),
                    AttributeValue::S("shop#details".to_owned()),
                );
                columns
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
            .filter_map(extract_shop_id)
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

    async fn get_raw_shop_name_record(
        &self,
        raw_shop_name: &ShopName,
    ) -> Result<Option<RawShopNameRecord>, SdkError<GetItemError>> {
        let rec = self
            .client
            .get_item()
            .table_name(&self.table)
            .key(
                "pk",
                AttributeValue::S(raw_shop_name_record::mk_pk(raw_shop_name)),
            )
            .key(
                "sk",
                AttributeValue::S(raw_shop_name_record::mk_sk().to_owned()),
            )
            .send()
            .await?
            .item
            .map(serde_dynamo::from_item::<_, RawShopNameRecord>)
            .and_then(|record_res| match record_res {
                Ok(record) => Some(record),
                Err(err) => {
                    error!(
                        error = %err,
                        type = %std::any::type_name::<RawShopNameRecord>(),
                        "Failed deserializing."
                    );
                    None
                }
            });

        Ok(rec)
    }

    async fn put_raw_shop_name_record(
        &self,
        record: RawShopNameRecord,
    ) -> Result<PutItemOutput, SdkError<PutItemError, HttpResponse>> {
        let payload = serde_dynamo::to_item(record).map_err(SdkError::construction_failure)?;
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(payload))
            .send()
            .await
    }

    async fn query_shop_by_shopify_domain(
        &self,
        shopify_domain: &Domain,
    ) -> Result<Option<ShopRecord>, SdkError<QueryError, HttpResponse>> {
        let maybe_item = self
            .client
            .query()
            .table_name(&self.table)
            .index_name("gsi3")
            .key_condition_expression("#gsi3_pk = :gsi3_pk_val AND #gsi3_sk = :gsi3_sk_val")
            .expression_attribute_names("#gsi3_pk", "gsi3_pk")
            .expression_attribute_names("#gsi3_sk", "gsi3_sk")
            .expression_attribute_values(
                ":gsi3_pk_val",
                AttributeValue::S(shop_record::mk_gsi3_pk(shopify_domain)),
            )
            .expression_attribute_values(
                ":gsi3_sk_val",
                AttributeValue::S(shop_record::mk_gsi3_sk().to_owned()),
            )
            .limit(1)
            .send()
            .await?
            .items
            .unwrap_or_default()
            .into_iter()
            .next();

        let Some(item) = maybe_item else {
            return Ok(None);
        };

        serde_dynamo::from_item::<_, ShopRecord>(item)
            .map(Some)
            .map_err(SdkError::construction_failure)
    }
}

fn extract_shop_id(attr_map: HashMap<String, AttributeValue>) -> Option<ShopId> {
    let mut attr_map = attr_map;
    match attr_map.remove("pk") {
        Some(AttributeValue::S(mut key)) => {
            let shop_id_pat = "shop#shop_id#";
            if key.starts_with(shop_id_pat) {
                let shop_id_str = key.split_off(shop_id_pat.len());
                match ShopId::try_from(&shop_id_str) {
                    Ok(shop_id) => Some(shop_id),
                    Err(err) => {
                        error!(error = %err, "Failed parsing extracted ShopId '{shop_id_str}'. This is a bug.");
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
    use crate::dynamodb::repository::extract_shop_id;
    use aws_sdk_dynamodb::types::AttributeValue;
    use common::shop_id::ShopId;
    use rstest;
    use std::collections::HashMap;

    #[rstest::rstest]
    #[case([].into(), None)]
    #[case([("pk".into(), AttributeValue::S("foo".into()))].into(), None)]
    #[case([("pk".into(), AttributeValue::S("shop#shop_id#bar".into()))].into(), None)]
    #[case([("pk".into(), AttributeValue::S("shop#shop_id#2a48a17b-cc4f-4489-83cf-f3f215711047".into()))].into(), Some(ShopId::try_from("2a48a17b-cc4f-4489-83cf-f3f215711047").unwrap()))]
    #[trace]
    fn should_extract_shop_id(
        #[case] attr_map: HashMap<String, AttributeValue>,
        #[case] expected: Option<ShopId>,
    ) {
        let actual = extract_shop_id(attr_map);

        assert_eq!(expected, actual);
    }
}
