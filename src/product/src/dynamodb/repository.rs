use crate::dynamodb::product_event_record::ProductEventRecord;
use crate::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use crate::dynamodb::product_meta_record::{self, ProductMetaRecord};
use crate::dynamodb::product_record;
use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::SdkError;
use aws_sdk_dynamodb::operation::batch_write_item::{BatchWriteItemError, BatchWriteItemOutput};
use aws_sdk_dynamodb::operation::get_item::GetItemError;
use aws_sdk_dynamodb::operation::query::QueryError;
use aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError;
use aws_sdk_dynamodb::types::{AttributeValue, Put, TransactWriteItem};
use common::batch::Batch;
use common::product_id::{ProductId, ProductKey};
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::slug_id::SlugId;
use std::collections::HashMap;
use tracing::error;

#[async_trait]
#[allow(clippy::result_large_err)]
#[mockall::automock]
pub trait ProductDynamoDbRepository {
    fn table(&self) -> &str;

    async fn put_product_event_records(
        &self,
        product_event_records: Batch<ProductEventRecord, 25>,
    ) -> Result<BatchWriteItemOutput, SdkError<BatchWriteItemError, HttpResponse>>;

    async fn transact_write_product_event_records(
        &self,
        product_event_records: Vec<ProductEventRecord>,
        product_meta_record: ProductMetaRecord,
        expected_event_version: u64,
    ) -> Result<(), SdkError<TransactWriteItemsError, HttpResponse>>;

    async fn query_product_event_records(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<Vec<ProductEventRecord>, SdkError<QueryError, HttpResponse>>;

    async fn query_product_domain_event_records(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<Vec<ProductDomainEventRecord>, SdkError<QueryError, HttpResponse>>;

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
    fn table(&self) -> &str {
        &self.table
    }

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

    async fn transact_write_product_event_records(
        &self,
        product_event_records: Vec<ProductEventRecord>,
        product_meta_record: ProductMetaRecord,
        expected_event_version: u64,
    ) -> Result<(), SdkError<TransactWriteItemsError, HttpResponse>> {
        let mut items = Vec::with_capacity(product_event_records.len() + 1);
        for event_record in product_event_records {
            let item =
                serde_dynamo::to_item(event_record).map_err(SdkError::construction_failure)?;
            let put = Put::builder()
                .table_name(&self.table)
                .set_item(Some(item))
                .condition_expression("attribute_not_exists(#pk) AND attribute_not_exists(#sk)")
                .expression_attribute_names("#pk", "pk")
                .expression_attribute_names("#sk", "sk")
                .build()
                .map_err(SdkError::construction_failure)?;
            items.push(TransactWriteItem::builder().put(put).build());
        }

        let meta_item =
            serde_dynamo::to_item(product_meta_record).map_err(SdkError::construction_failure)?;
        let condition_expression = if expected_event_version == 0 {
            "attribute_not_exists(#pk) AND attribute_not_exists(#sk)"
        } else {
            "#event_version = :expected_event_version"
        };
        let mut put_builder = Put::builder()
            .table_name(&self.table)
            .set_item(Some(meta_item))
            .condition_expression(condition_expression)
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#sk", "sk");
        if expected_event_version != 0 {
            put_builder = put_builder
                .expression_attribute_names("#event_version", "event_version")
                .expression_attribute_values(
                    ":expected_event_version",
                    AttributeValue::N(expected_event_version.to_string()),
                );
        }
        let put = put_builder
            .build()
            .map_err(SdkError::construction_failure)?;
        items.push(TransactWriteItem::builder().put(put).build());

        self.client
            .transact_write_items()
            .set_transact_items(Some(items))
            .send()
            .await
            .map(|_| ())
    }

    async fn query_product_event_records(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<Vec<ProductEventRecord>, SdkError<QueryError, HttpResponse>> {
        let pk = product_record::mk_pk(shop_id, shops_product_id);
        let mut start_key = None;
        let mut items = Vec::new();
        loop {
            let query_output = self
                .client
                .query()
                .table_name(&self.table)
                .key_condition_expression("#pk = :pk_val AND begins_with(#sk, :sk_prefix)")
                .expression_attribute_names("#pk", "pk")
                .expression_attribute_names("#sk", "sk")
                .expression_attribute_values(":pk_val", AttributeValue::S(pk.clone()))
                .expression_attribute_values(
                    ":sk_prefix",
                    AttributeValue::S("product#event#".to_string()),
                )
                // Product replay depends on applying events in sort-key order.
                .scan_index_forward(true)
                .set_exclusive_start_key(start_key)
                .send()
                .await?;
            items.extend(query_output.items.unwrap_or_default());
            start_key = query_output.last_evaluated_key;
            if start_key.is_none() {
                break;
            }
        }

        Ok(items
            .into_iter()
            .filter_map(|item| match serde_dynamo::from_item::<_, ProductEventRecord>(item) {
                Ok(record) => Some(record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<ProductEventRecord>(), "Failed deserializing ProductEventRecord.");
                    None
                }
            })
            .collect())
    }

    async fn query_product_domain_event_records(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<Vec<ProductDomainEventRecord>, SdkError<QueryError, HttpResponse>> {
        Ok(self
            .query_product_event_records(shop_id, shops_product_id)
            .await?
            .into_iter()
            .filter_map(|record| match record {
                ProductEventRecord::Domain(record) => Some(record),
                _ => None,
            })
            .collect())
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
            .key(
                "sk",
                AttributeValue::S(product_meta_record::mk_sk().to_owned()),
            )
            .projection_expression("product_id")
            .send()
            .await?
            .item
            .map(extract_product_id)
            .transpose()
            .map_err(SdkError::construction_failure)?;

        Ok(product_id)
    }

    async fn query_product_key(
        &self,
        shop_slug_id: &SlugId<0>,
        product_slug_id: &SlugId<6>,
    ) -> Result<Option<ProductKey>, SdkError<QueryError, HttpResponse>> {
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
            .next();

        let Some(mut attr_map) = maybe_item else {
            return Ok(None);
        };

        let pk_attr = attr_map.remove("pk").ok_or_else(|| {
            SdkError::construction_failure(format!(
                "DynamoDB Response attribute-map did not contain field 'pk' when querying gsi2 \
                for shop_slug_id '{shop_slug_id}' and product_slug_id '{product_slug_id}'"
            ))
        })?;

        let pk_str = pk_attr
            .as_s()
            .expect(
                "shouldn't fail extracting 'pk' as String because PKs are always Strings for us",
            )
            .clone();

        ProductKey::try_from(pk_str.trim_start_matches("product#"))
            .map(Some)
            .map_err(SdkError::construction_failure)
    }
}

pub fn extract_product_key(map: HashMap<String, AttributeValue>) -> Result<ProductKey, String> {
    let mut map = map;
    if let Some(pk_attr) = map.remove("pk") {
        let product_key = pk_attr
            .as_s()
            .map_err(|_| format!("Extracted value for pk '{pk_attr:?}' failed."))?;
        ProductKey::try_from(product_key.trim_start_matches("product#"))
            .map_err(|err| err.to_string())
    } else {
        Err(format!(
            "AttributeValue-Map does not contain key pk: '{map:?}'."
        ))
    }
}

pub fn extract_product_id(
    map: HashMap<String, AttributeValue>,
) -> Result<ProductId, Box<dyn std::error::Error + Send + Sync>> {
    let mut map = map;
    if let Some(product_id_attr) = map.remove("product_id") {
        let product_id_str = product_id_attr
            .as_s()
            .map_err(|_| format!("Extracted value for product_id '{product_id_attr:?}' failed."))?;
        Ok(ProductId::try_from(product_id_str.as_str())?)
    } else {
        Err(format!("AttributeValue-Map does not contain key product_id: '{map:?}'.").into())
    }
}
