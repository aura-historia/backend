use crate::dynamodb::product_event_record::ProductEventRecord;
use crate::dynamodb::product_event_record::domain::{self, ProductDomainEventRecord};
use crate::dynamodb::product_meta_record::ProductMetaRecord;
use crate::dynamodb::product_record::{self, ProductRecord};
use crate::dynamodb::repository::ProductDynamoDbRepository;
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::SdkError;
use aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError;
use aws_sdk_dynamodb::types::AttributeValue;
use std::collections::HashMap;

pub struct SeedProductRecordsOutput {
    pub unprocessed_items: Option<HashMap<String, Vec<AttributeValue>>>,
}

pub fn product_record_to_created_event_record(record: &ProductRecord) -> ProductDomainEventRecord {
    ProductDomainEventRecord {
        pk: domain::mk_pk(&record.shop_id, &record.shops_product_id),
        sk: domain::mk_sk(&record.event_id),
        product_id: record.product_id,
        product_slug_id: Some(record.product_slug_id.clone()),
        shop_slug_id: Some(record.shop_slug_id.clone()),
        seller_slug_id: Some(record.seller_slug_id.clone()),
        event_id: record.event_id,
        event_type: crate::dynamodb::product_event_type_record::domain::ProductDomainEventTypeRecord::DomainCreated,
        event_type_schema_version: 0,
        shop_id: record.shop_id,
        seller_id: record.seller_id,
        shops_product_id: record.shops_product_id.clone(),
        shop_name: Some(record.shop_name.clone()),
        seller_name: Some(record.seller_name.clone()),
        shop_type: Some(record.shop_type),
        structured_address_addressline: record.structured_address_addressline.clone(),
        structured_address_addressline_extra: record.structured_address_addressline_extra.clone(),
        structured_address_locality: record.structured_address_locality.clone(),
        structured_address_region: record.structured_address_region.clone(),
        structured_address_postal_code: record.structured_address_postal_code.clone(),
        structured_address_country: record.structured_address_country,
        geo_address_lat: record.geo_address_lat,
        geo_address_lon: record.geo_address_lon,
        title_native: Some(record.title_native.clone()),
        title_de: record.title_de.clone(),
        title_en: record.title_en.clone(),
        title_fr: record.title_fr.clone(),
        title_es: record.title_es.clone(),
        title_it: record.title_it.clone(),
        description_native: record.description_native.clone(),
        new_price_native: record.price_native,
        new_price_eur: record.price_eur,
        new_price_usd: record.price_usd,
        new_price_gbp: record.price_gbp,
        new_price_aud: record.price_aud,
        new_price_cad: record.price_cad,
        new_price_nzd: record.price_nzd,
        new_price_cny: record.price_cny,
        new_price_brl: record.price_brl,
        new_price_pln: record.price_pln,
        new_price_try: record.price_try,
        new_price_jpy: record.price_jpy,
        new_price_czk: record.price_czk,
        new_price_rub: record.price_rub,
        new_price_aed: record.price_aed,
        new_price_sar: record.price_sar,
        new_price_hkd: record.price_hkd,
        new_price_sgd: record.price_sgd,
        new_price_chf: record.price_chf,
        new_price_estimate_min_native: record.price_estimate_min_native,
        new_price_estimate_min_eur: record.price_estimate_min_eur,
        new_price_estimate_min_usd: record.price_estimate_min_usd,
        new_price_estimate_min_gbp: record.price_estimate_min_gbp,
        new_price_estimate_min_aud: record.price_estimate_min_aud,
        new_price_estimate_min_cad: record.price_estimate_min_cad,
        new_price_estimate_min_nzd: record.price_estimate_min_nzd,
        new_price_estimate_min_cny: record.price_estimate_min_cny,
        new_price_estimate_min_brl: record.price_estimate_min_brl,
        new_price_estimate_min_pln: record.price_estimate_min_pln,
        new_price_estimate_min_try: record.price_estimate_min_try,
        new_price_estimate_min_jpy: record.price_estimate_min_jpy,
        new_price_estimate_min_czk: record.price_estimate_min_czk,
        new_price_estimate_min_rub: record.price_estimate_min_rub,
        new_price_estimate_min_aed: record.price_estimate_min_aed,
        new_price_estimate_min_sar: record.price_estimate_min_sar,
        new_price_estimate_min_hkd: record.price_estimate_min_hkd,
        new_price_estimate_min_sgd: record.price_estimate_min_sgd,
        new_price_estimate_min_chf: record.price_estimate_min_chf,
        new_price_estimate_max_native: record.price_estimate_max_native,
        new_price_estimate_max_eur: record.price_estimate_max_eur,
        new_price_estimate_max_usd: record.price_estimate_max_usd,
        new_price_estimate_max_gbp: record.price_estimate_max_gbp,
        new_price_estimate_max_aud: record.price_estimate_max_aud,
        new_price_estimate_max_cad: record.price_estimate_max_cad,
        new_price_estimate_max_nzd: record.price_estimate_max_nzd,
        new_price_estimate_max_cny: record.price_estimate_max_cny,
        new_price_estimate_max_brl: record.price_estimate_max_brl,
        new_price_estimate_max_pln: record.price_estimate_max_pln,
        new_price_estimate_max_try: record.price_estimate_max_try,
        new_price_estimate_max_jpy: record.price_estimate_max_jpy,
        new_price_estimate_max_czk: record.price_estimate_max_czk,
        new_price_estimate_max_rub: record.price_estimate_max_rub,
        new_price_estimate_max_aed: record.price_estimate_max_aed,
        new_price_estimate_max_sar: record.price_estimate_max_sar,
        new_price_estimate_max_hkd: record.price_estimate_max_hkd,
        new_price_estimate_max_sgd: record.price_estimate_max_sgd,
        new_price_estimate_max_chf: record.price_estimate_max_chf,
        old_price_native: None,
        old_price_eur: None,
        old_price_usd: None,
        old_price_gbp: None,
        old_price_aud: None,
        old_price_cad: None,
        old_price_nzd: None,
        old_price_cny: None,
        old_price_brl: None,
        old_price_pln: None,
        old_price_try: None,
        old_price_jpy: None,
        old_price_czk: None,
        old_price_rub: None,
        old_price_aed: None,
        old_price_sar: None,
        old_price_hkd: None,
        old_price_sgd: None,
        old_price_chf: None,
        new_state: Some(record.state),
        old_state: None,
        url: Some(record.url.clone()),
        images: Some(record.images.clone()),
        auction_start: record.auction_start,
        auction_end: record.auction_end,
        timestamp: record.created,
    }
}

pub fn product_record_to_meta_record(
    record: &ProductRecord,
    event_version: u64,
) -> ProductMetaRecord {
    ProductMetaRecord {
        pk: product_record::mk_pk(&record.shop_id, &record.shops_product_id),
        sk: crate::dynamodb::product_meta_record::mk_sk().to_owned(),
        gsi2_pk: product_record::mk_gsi2_pk(&record.shop_slug_id, &record.product_slug_id),
        gsi2_sk: product_record::mk_gsi2_sk().to_owned(),
        product_id: record.product_id,
        product_slug_id: record.product_slug_id.clone(),
        shop_slug_id: record.shop_slug_id.clone(),
        seller_slug_id: record.seller_slug_id.clone(),
        shop_id: record.shop_id,
        shops_product_id: record.shops_product_id.clone(),
        event_version,
    }
}

pub async fn transact_write_product_record_as_events(
    repository: &(impl ProductDynamoDbRepository + Sync),
    record: ProductRecord,
) -> Result<(), SdkError<TransactWriteItemsError, HttpResponse>> {
    repository
        .transact_write_product_event_records(
            vec![ProductEventRecord::Domain(
                product_record_to_created_event_record(&record),
            )],
            product_record_to_meta_record(&record, 1),
            0,
        )
        .await
}

pub async fn transact_write_product_records_as_events(
    repository: &(impl ProductDynamoDbRepository + Sync),
    records: impl IntoIterator<Item = ProductRecord>,
) -> Result<(), SdkError<TransactWriteItemsError, HttpResponse>> {
    let records = records.into_iter().collect::<Vec<_>>();
    for record in records {
        transact_write_product_record_as_events(repository, record).await?;
    }
    Ok(())
}

#[async_trait::async_trait]
pub trait ProductRecordSeedExt {
    async fn transact_write_product_records_as_events(
        &self,
        records: impl IntoIterator<Item = ProductRecord> + Send,
    ) -> Result<SeedProductRecordsOutput, SdkError<TransactWriteItemsError, HttpResponse>>;
}

#[async_trait::async_trait]
impl<T> ProductRecordSeedExt for T
where
    T: ProductDynamoDbRepository + Sync,
{
    async fn transact_write_product_records_as_events(
        &self,
        records: impl IntoIterator<Item = ProductRecord> + Send,
    ) -> Result<SeedProductRecordsOutput, SdkError<TransactWriteItemsError, HttpResponse>> {
        transact_write_product_records_as_events(self, records).await?;
        Ok(SeedProductRecordsOutput {
            unprocessed_items: Some(HashMap::new()),
        })
    }
}
