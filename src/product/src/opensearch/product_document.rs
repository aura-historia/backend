use std::collections::HashMap;

use crate::core::origin_year::OriginYear;
use crate::core::product::Product;
use crate::core::product_image::ProductImage;
use crate::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use crate::dynamodb::product_record::ProductRecord;
use crate::opensearch::authenticity_document::AuthenticityDocument;
use crate::opensearch::condition_document::ConditionDocument;
use crate::opensearch::product_image_document::ProductImageDocument;
use crate::opensearch::product_state_document::ProductStateDocument;
use crate::opensearch::provenance_document::ProvenanceDocument;
use crate::opensearch::restoration_document::RestorationDocument;
use common::category_key::CategoryId;
use common::currency::domain::Currency;
use common::error::mapping_error::PersistenceMappingError;
use common::error::missing_field::MissingPersistenceField;
use common::language::document::TextDocument;
use common::language::domain::Language;
use common::localized::Localized;
use common::period_key::PeriodId;
use common::product_id::{ProductId, ProductKey};
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::slug_id::SlugId;
use common::utm::append_utm_params;
use common::year::{Year, YearRange};
use common::{event_id::EventId, has_key::HasKey};
use field::field;
use geo::core::continent::Continent;
use geo::opensearch::{
    geo_address_from_document, geo_address_to_document, structured_address_from_document,
};
use isocountry::CountryCode;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use shop::opensearch::{
    continent_document::ContinentDocument, shop_type_document::ShopTypeDocument,
};
use strum::EnumCount;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
#[serde(rename_all = "camelCase")]
pub struct ProductDocument {
    pub product_id: ProductId,
    pub product_slug_id: SlugId<6>,
    pub shop_slug_id: SlugId<0>,
    pub seller_slug_id: SlugId<0>,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: String,
    pub seller_name: String,
    pub shop_type: ShopTypeDocument,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_addressline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_addressline_extra: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_locality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_postal_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_country: Option<CountryCode>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address_continent: Option<ContinentDocument>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub geo_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub category_id: Option<CategoryId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub period_id: Option<PeriodId>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub category_name_de: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub category_name_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub category_name_fr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub category_name_es: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub category_name_it: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub period_name_de: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub period_name_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub period_name_fr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub period_name_es: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub period_name_it: Option<String>,

    pub title_native: TextDocument,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_de: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_fr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_es: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_it: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_eur: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_usd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_gbp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_aud: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_cad: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_nzd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_cny: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_brl: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_pln: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_try: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_jpy: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_czk: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_rub: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_aed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_sar: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_hkd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_sgd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_chf: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_eur: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_usd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_gbp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_aud: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_cad: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_nzd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_cny: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_brl: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_pln: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_try: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_jpy: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_czk: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_rub: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_aed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_sar: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_hkd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_sgd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min_chf: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_eur: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_usd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_gbp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_aud: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_cad: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_nzd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_cny: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_brl: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_pln: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_try: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_jpy: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_czk: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_rub: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_aed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_sar: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_hkd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_sgd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max_chf: Option<u64>,

    pub state: ProductStateDocument,
    pub url: Url,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub images: Vec<ProductImageDocument>,

    // dim=768 via google/gemini-embedding-2
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub embedding: Option<Vec<f32>>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_year_min: Option<Year>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_year: Option<Year>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_year_max: Option<Year>,

    #[serde(default)]
    pub authenticity: AuthenticityDocument,
    #[serde(default)]
    pub condition: ConditionDocument,
    #[serde(default)]
    pub provenance: ProvenanceDocument,
    #[serde(default)]
    pub restoration: RestorationDocument,

    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub auction_start: Option<OffsetDateTime>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub auction_end: Option<OffsetDateTime>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl ProductDocument {
    pub fn _id(&self) -> ProductId {
        self.product_id
    }
}

impl HasKey for ProductDocument {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        ProductKey {
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id.clone(),
        }
    }
}

impl TryFrom<ProductDomainEventRecord> for ProductDocument {
    type Error = PersistenceMappingError;

    fn try_from(event_product_document: ProductDomainEventRecord) -> Result<Self, Self::Error> {
        let state = event_product_document
            .new_state
            .map(ProductStateDocument::from)
            .ok_or_else(|| {
                MissingPersistenceField::new(field!(new_state@ProductDomainEventRecord))
            })?;
        let document = ProductDocument {
            product_id: event_product_document.product_id,
            product_slug_id: event_product_document.product_slug_id.ok_or_else(|| {
                MissingPersistenceField::new(field!(product_slug_id@ProductDomainEventRecord))
            })?,
            shop_slug_id: event_product_document.shop_slug_id.ok_or_else(|| {
                MissingPersistenceField::new(field!(shop_slug_id@ProductDomainEventRecord))
            })?,
            seller_slug_id: event_product_document.seller_slug_id.ok_or_else(|| {
                MissingPersistenceField::new(field!(seller_slug_id@ProductDomainEventRecord))
            })?,
            event_id: event_product_document.event_id,
            shop_id: event_product_document.shop_id,
            seller_id: event_product_document.seller_id,
            shops_product_id: event_product_document.shops_product_id,
            shop_name: event_product_document.shop_name.ok_or_else(|| {
                MissingPersistenceField::new(field!(shop_name@ProductDomainEventRecord))
            })?,
            seller_name: event_product_document.seller_name.ok_or_else(|| {
                MissingPersistenceField::new(field!(seller_name@ProductDomainEventRecord))
            })?,
            shop_type: event_product_document
                .shop_type
                .map(Into::into)
                .ok_or_else(|| {
                    MissingPersistenceField::new(field!(shop_type@ProductDomainEventRecord))
                })?,
            structured_address_addressline: event_product_document.structured_address_addressline,
            structured_address_addressline_extra: event_product_document
                .structured_address_addressline_extra,
            structured_address_locality: event_product_document.structured_address_locality,
            structured_address_region: event_product_document.structured_address_region,
            structured_address_postal_code: event_product_document.structured_address_postal_code,
            structured_address_country: event_product_document.structured_address_country,
            structured_address_continent: event_product_document
                .structured_address_country
                .map(|country| ContinentDocument::from(Continent::from(country))),
            geo_address: event_product_document
                .geo_address_lat
                .zip(event_product_document.geo_address_lon)
                .map(|(lat, lon)| format!("{lat},{lon}")),
            category_id: None,
            period_id: None,
            category_name_de: None,
            category_name_en: None,
            category_name_fr: None,
            category_name_es: None,
            category_name_it: None,
            period_name_de: None,
            period_name_en: None,
            period_name_fr: None,
            period_name_es: None,
            period_name_it: None,
            title_native: event_product_document
                .title_native
                .map(TextDocument::from)
                .ok_or_else(|| {
                    MissingPersistenceField::new(field!(title_native@ProductDomainEventRecord))
                })?,
            title_de: event_product_document.title_de,
            title_en: event_product_document.title_en,
            title_fr: event_product_document.title_fr,
            title_es: event_product_document.title_es,
            title_it: event_product_document.title_it,
            price_eur: event_product_document.new_price_eur,
            price_usd: event_product_document.new_price_usd,
            price_gbp: event_product_document.new_price_gbp,
            price_aud: event_product_document.new_price_aud,
            price_cad: event_product_document.new_price_cad,
            price_nzd: event_product_document.new_price_nzd,
            price_cny: event_product_document.new_price_cny,
            price_brl: event_product_document.new_price_brl,
            price_pln: event_product_document.new_price_pln,
            price_try: event_product_document.new_price_try,
            price_jpy: event_product_document.new_price_jpy,
            price_czk: event_product_document.new_price_czk,
            price_rub: event_product_document.new_price_rub,
            price_aed: event_product_document.new_price_aed,
            price_sar: event_product_document.new_price_sar,
            price_hkd: event_product_document.new_price_hkd,
            price_sgd: event_product_document.new_price_sgd,
            price_chf: event_product_document.new_price_chf,
            price_estimate_min_eur: event_product_document.new_price_estimate_min_eur,
            price_estimate_min_usd: event_product_document.new_price_estimate_min_usd,
            price_estimate_min_gbp: event_product_document.new_price_estimate_min_gbp,
            price_estimate_min_aud: event_product_document.new_price_estimate_min_aud,
            price_estimate_min_cad: event_product_document.new_price_estimate_min_cad,
            price_estimate_min_nzd: event_product_document.new_price_estimate_min_nzd,
            price_estimate_min_cny: event_product_document.new_price_estimate_min_cny,
            price_estimate_min_brl: event_product_document.new_price_estimate_min_brl,
            price_estimate_min_pln: event_product_document.new_price_estimate_min_pln,
            price_estimate_min_try: event_product_document.new_price_estimate_min_try,
            price_estimate_min_jpy: event_product_document.new_price_estimate_min_jpy,
            price_estimate_min_czk: event_product_document.new_price_estimate_min_czk,
            price_estimate_min_rub: event_product_document.new_price_estimate_min_rub,
            price_estimate_min_aed: event_product_document.new_price_estimate_min_aed,
            price_estimate_min_sar: event_product_document.new_price_estimate_min_sar,
            price_estimate_min_hkd: event_product_document.new_price_estimate_min_hkd,
            price_estimate_min_sgd: event_product_document.new_price_estimate_min_sgd,
            price_estimate_min_chf: event_product_document.new_price_estimate_min_chf,
            price_estimate_max_eur: event_product_document.new_price_estimate_max_eur,
            price_estimate_max_usd: event_product_document.new_price_estimate_max_usd,
            price_estimate_max_gbp: event_product_document.new_price_estimate_max_gbp,
            price_estimate_max_aud: event_product_document.new_price_estimate_max_aud,
            price_estimate_max_cad: event_product_document.new_price_estimate_max_cad,
            price_estimate_max_nzd: event_product_document.new_price_estimate_max_nzd,
            price_estimate_max_cny: event_product_document.new_price_estimate_max_cny,
            price_estimate_max_brl: event_product_document.new_price_estimate_max_brl,
            price_estimate_max_pln: event_product_document.new_price_estimate_max_pln,
            price_estimate_max_try: event_product_document.new_price_estimate_max_try,
            price_estimate_max_jpy: event_product_document.new_price_estimate_max_jpy,
            price_estimate_max_czk: event_product_document.new_price_estimate_max_czk,
            price_estimate_max_rub: event_product_document.new_price_estimate_max_rub,
            price_estimate_max_aed: event_product_document.new_price_estimate_max_aed,
            price_estimate_max_sar: event_product_document.new_price_estimate_max_sar,
            price_estimate_max_hkd: event_product_document.new_price_estimate_max_hkd,
            price_estimate_max_sgd: event_product_document.new_price_estimate_max_sgd,
            price_estimate_max_chf: event_product_document.new_price_estimate_max_chf,
            state,
            url: event_product_document.url.ok_or_else(|| {
                MissingPersistenceField::new(field!(url@ProductDomainEventRecord))
            })?,
            images: event_product_document
                .images
                .unwrap_or_default()
                .into_iter()
                .map(ProductImageDocument::from)
                .collect(),
            embedding: None,
            origin_year_min: None,
            origin_year: None,
            origin_year_max: None,
            authenticity: Default::default(),
            condition: Default::default(),
            provenance: Default::default(),
            restoration: Default::default(),
            auction_start: event_product_document.auction_start,
            auction_end: event_product_document.auction_end,
            created: event_product_document.timestamp,
            updated: event_product_document.timestamp,
        };
        Ok(document)
    }
}

impl From<ProductRecord> for ProductDocument {
    fn from(product_document: ProductRecord) -> Self {
        ProductDocument {
            product_id: product_document.product_id,
            product_slug_id: product_document.product_slug_id,
            shop_slug_id: product_document.shop_slug_id,
            seller_slug_id: product_document.seller_slug_id,
            event_id: product_document.event_id,
            shop_id: product_document.shop_id,
            seller_id: product_document.seller_id,
            shops_product_id: product_document.shops_product_id,
            shop_name: product_document.shop_name,
            seller_name: product_document.seller_name,
            shop_type: product_document.shop_type.into(),
            structured_address_addressline: product_document.structured_address_addressline,
            structured_address_addressline_extra: product_document
                .structured_address_addressline_extra,
            structured_address_locality: product_document.structured_address_locality,
            structured_address_region: product_document.structured_address_region,
            structured_address_postal_code: product_document.structured_address_postal_code,
            structured_address_country: product_document.structured_address_country,
            structured_address_continent: product_document
                .structured_address_country
                .map(|country| ContinentDocument::from(Continent::from(country))),
            geo_address: product_document
                .geo_address_lat
                .zip(product_document.geo_address_lon)
                .map(|(lat, lon)| format!("{lat},{lon}")),
            category_id: product_document.category_id,
            period_id: product_document.period_id,
            category_name_de: product_document.category_name_de,
            category_name_en: product_document.category_name_en,
            category_name_fr: product_document.category_name_fr,
            category_name_es: product_document.category_name_es,
            category_name_it: product_document.category_name_it,
            period_name_de: product_document.period_name_de,
            period_name_en: product_document.period_name_en,
            period_name_fr: product_document.period_name_fr,
            period_name_es: product_document.period_name_es,
            period_name_it: product_document.period_name_it,
            title_native: product_document.title_native.into(),
            title_de: product_document.title_de,
            title_en: product_document.title_en,
            title_fr: product_document.title_fr,
            title_es: product_document.title_es,
            title_it: product_document.title_it,
            price_eur: product_document.price_eur,
            price_usd: product_document.price_usd,
            price_gbp: product_document.price_gbp,
            price_aud: product_document.price_aud,
            price_cad: product_document.price_cad,
            price_nzd: product_document.price_nzd,
            price_cny: product_document.price_cny,
            price_brl: product_document.price_brl,
            price_pln: product_document.price_pln,
            price_try: product_document.price_try,
            price_jpy: product_document.price_jpy,
            price_czk: product_document.price_czk,
            price_rub: product_document.price_rub,
            price_aed: product_document.price_aed,
            price_sar: product_document.price_sar,
            price_hkd: product_document.price_hkd,
            price_sgd: product_document.price_sgd,
            price_chf: product_document.price_chf,
            price_estimate_min_eur: product_document.price_estimate_min_eur,
            price_estimate_min_usd: product_document.price_estimate_min_usd,
            price_estimate_min_gbp: product_document.price_estimate_min_gbp,
            price_estimate_min_aud: product_document.price_estimate_min_aud,
            price_estimate_min_cad: product_document.price_estimate_min_cad,
            price_estimate_min_nzd: product_document.price_estimate_min_nzd,
            price_estimate_min_cny: product_document.price_estimate_min_cny,
            price_estimate_min_brl: product_document.price_estimate_min_brl,
            price_estimate_min_pln: product_document.price_estimate_min_pln,
            price_estimate_min_try: product_document.price_estimate_min_try,
            price_estimate_min_jpy: product_document.price_estimate_min_jpy,
            price_estimate_min_czk: product_document.price_estimate_min_czk,
            price_estimate_min_rub: product_document.price_estimate_min_rub,
            price_estimate_min_aed: product_document.price_estimate_min_aed,
            price_estimate_min_sar: product_document.price_estimate_min_sar,
            price_estimate_min_hkd: product_document.price_estimate_min_hkd,
            price_estimate_min_sgd: product_document.price_estimate_min_sgd,
            price_estimate_min_chf: product_document.price_estimate_min_chf,
            price_estimate_max_eur: product_document.price_estimate_max_eur,
            price_estimate_max_usd: product_document.price_estimate_max_usd,
            price_estimate_max_gbp: product_document.price_estimate_max_gbp,
            price_estimate_max_aud: product_document.price_estimate_max_aud,
            price_estimate_max_cad: product_document.price_estimate_max_cad,
            price_estimate_max_nzd: product_document.price_estimate_max_nzd,
            price_estimate_max_cny: product_document.price_estimate_max_cny,
            price_estimate_max_brl: product_document.price_estimate_max_brl,
            price_estimate_max_pln: product_document.price_estimate_max_pln,
            price_estimate_max_try: product_document.price_estimate_max_try,
            price_estimate_max_jpy: product_document.price_estimate_max_jpy,
            price_estimate_max_czk: product_document.price_estimate_max_czk,
            price_estimate_max_rub: product_document.price_estimate_max_rub,
            price_estimate_max_aed: product_document.price_estimate_max_aed,
            price_estimate_max_sar: product_document.price_estimate_max_sar,
            price_estimate_max_hkd: product_document.price_estimate_max_hkd,
            price_estimate_max_sgd: product_document.price_estimate_max_sgd,
            price_estimate_max_chf: product_document.price_estimate_max_chf,
            state: product_document.state.into(),
            url: product_document.url,
            images: product_document
                .images
                .into_iter()
                .map(ProductImageDocument::from)
                .collect(),
            embedding: None,
            origin_year_min: product_document.origin_year_min,
            origin_year: product_document.origin_year,
            origin_year_max: product_document.origin_year_max,
            authenticity: product_document.authenticity.into(),
            condition: product_document.condition.into(),
            provenance: product_document.provenance.into(),
            restoration: product_document.restoration.into(),
            auction_start: product_document.auction_start,
            auction_end: product_document.auction_end,
            created: product_document.created,
            updated: product_document.updated,
        }
    }
}

impl ProductDocumentSerdeField {
    pub fn description_fields() -> Vec<ProductDocumentSerdeField> {
        [
            ProductDocumentSerdeField::DescriptionDe,
            ProductDocumentSerdeField::DescriptionEn,
            ProductDocumentSerdeField::DescriptionFr,
            ProductDocumentSerdeField::DescriptionEs,
            ProductDocumentSerdeField::DescriptionIt,
        ]
        .into()
    }
}

impl From<Product> for ProductDocument {
    fn from(product: Product) -> Self {
        let mut product = product;

        let category_name_de = product
            .category_name
            .remove(&Language::De)
            .map(String::from);
        let category_name_en = product
            .category_name
            .remove(&Language::En)
            .map(String::from);
        let category_name_fr = product
            .category_name
            .remove(&Language::Fr)
            .map(String::from);
        let category_name_es = product
            .category_name
            .remove(&Language::Es)
            .map(String::from);
        let category_name_it = product
            .category_name
            .remove(&Language::It)
            .map(String::from);

        let period_name_de = product.period_name.remove(&Language::De).map(String::from);
        let period_name_en = product.period_name.remove(&Language::En).map(String::from);
        let period_name_fr = product.period_name.remove(&Language::Fr).map(String::from);
        let period_name_es = product.period_name.remove(&Language::Es).map(String::from);
        let period_name_it = product.period_name.remove(&Language::It).map(String::from);

        let title_de = product.other_title.remove(&Language::De).map(String::from);
        let title_en = product.other_title.remove(&Language::En).map(String::from);
        let title_fr = product.other_title.remove(&Language::Fr).map(String::from);
        let title_es = product.other_title.remove(&Language::Es).map(String::from);
        let title_it = product.other_title.remove(&Language::It).map(String::from);

        let (origin_year_min, origin_year, origin_year_max) = match product.origin_year {
            Some(OriginYear::ExactYear(y)) => (None, Some(y), None),
            Some(OriginYear::EstimatedRange(range)) => (range.min, None, range.max),
            None => (None, None, None),
        };

        ProductDocument {
            product_id: product.product_id,
            product_slug_id: product.product_slug_id,
            shop_slug_id: product.shop_slug_id,
            seller_slug_id: product.seller_slug_id,
            event_id: product.event_id,
            shop_id: product.shop_id,
            seller_id: product.seller_id,
            shops_product_id: product.shops_product_id,
            shop_name: String::from(product.shop_name),
            seller_name: String::from(product.seller_name),
            shop_type: product.shop_type.into(),
            structured_address_addressline: product
                .structured_address
                .as_ref()
                .and_then(|address| address.addressline.clone()),
            structured_address_addressline_extra: product
                .structured_address
                .as_ref()
                .and_then(|address| address.addressline_extra.clone()),
            structured_address_locality: product
                .structured_address
                .as_ref()
                .and_then(|address| address.locality.clone()),
            structured_address_region: product
                .structured_address
                .as_ref()
                .and_then(|address| address.region.clone()),
            structured_address_postal_code: product
                .structured_address
                .as_ref()
                .and_then(|address| address.postal_code.clone()),
            structured_address_country: product
                .structured_address
                .as_ref()
                .and_then(|address| address.country),
            structured_address_continent: product
                .structured_address
                .as_ref()
                .and_then(|address| address.country)
                .map(|country| ContinentDocument::from(Continent::from(country))),
            geo_address: geo_address_to_document(product.geo_address),
            category_id: product.category_id,
            period_id: product.period_id,
            category_name_de,
            category_name_en,
            category_name_fr,
            category_name_es,
            category_name_it,
            period_name_de,
            period_name_en,
            period_name_fr,
            period_name_es,
            period_name_it,
            title_native: product.native_title.into(),
            title_de,
            title_en,
            title_fr,
            title_es,
            title_it,
            price_eur: Currency::Eur
                .extract_amount(&product.native_price, &product.other_price)
                .map(u64::from),
            price_usd: Currency::Usd
                .extract_amount(&product.native_price, &product.other_price)
                .map(u64::from),
            price_gbp: Currency::Gbp
                .extract_amount(&product.native_price, &product.other_price)
                .map(u64::from),
            price_aud: Currency::Aud
                .extract_amount(&product.native_price, &product.other_price)
                .map(u64::from),
            price_cad: Currency::Cad
                .extract_amount(&product.native_price, &product.other_price)
                .map(u64::from),
            price_nzd: Currency::Nzd
                .extract_amount(&product.native_price, &product.other_price)
                .map(u64::from),
            price_cny: Currency::Cny
                .extract_amount(&product.native_price, &product.other_price)
                .map(u64::from),
            price_brl: Currency::Brl
                .extract_amount(&product.native_price, &product.other_price)
                .map(u64::from),
            price_pln: Currency::Pln
                .extract_amount(&product.native_price, &product.other_price)
                .map(u64::from),
            price_try: Currency::Try
                .extract_amount(&product.native_price, &product.other_price)
                .map(u64::from),
            price_jpy: Currency::Jpy
                .extract_amount(&product.native_price, &product.other_price)
                .map(u64::from),
            price_czk: Currency::Czk
                .extract_amount(&product.native_price, &product.other_price)
                .map(u64::from),
            price_rub: Currency::Rub
                .extract_amount(&product.native_price, &product.other_price)
                .map(u64::from),
            price_aed: Currency::Aed
                .extract_amount(&product.native_price, &product.other_price)
                .map(u64::from),
            price_sar: Currency::Sar
                .extract_amount(&product.native_price, &product.other_price)
                .map(u64::from),
            price_hkd: Currency::Hkd
                .extract_amount(&product.native_price, &product.other_price)
                .map(u64::from),
            price_sgd: Currency::Sgd
                .extract_amount(&product.native_price, &product.other_price)
                .map(u64::from),
            price_chf: Currency::Chf
                .extract_amount(&product.native_price, &product.other_price)
                .map(u64::from),
            price_estimate_min_eur: Currency::Eur
                .extract_amount(
                    &product.native_price_estimate_min,
                    &product.other_price_estimate_min,
                )
                .map(u64::from),
            price_estimate_min_usd: Currency::Usd
                .extract_amount(
                    &product.native_price_estimate_min,
                    &product.other_price_estimate_min,
                )
                .map(u64::from),
            price_estimate_min_gbp: Currency::Gbp
                .extract_amount(
                    &product.native_price_estimate_min,
                    &product.other_price_estimate_min,
                )
                .map(u64::from),
            price_estimate_min_aud: Currency::Aud
                .extract_amount(
                    &product.native_price_estimate_min,
                    &product.other_price_estimate_min,
                )
                .map(u64::from),
            price_estimate_min_cad: Currency::Cad
                .extract_amount(
                    &product.native_price_estimate_min,
                    &product.other_price_estimate_min,
                )
                .map(u64::from),
            price_estimate_min_nzd: Currency::Nzd
                .extract_amount(
                    &product.native_price_estimate_min,
                    &product.other_price_estimate_min,
                )
                .map(u64::from),
            price_estimate_min_cny: Currency::Cny
                .extract_amount(
                    &product.native_price_estimate_min,
                    &product.other_price_estimate_min,
                )
                .map(u64::from),
            price_estimate_min_brl: Currency::Brl
                .extract_amount(
                    &product.native_price_estimate_min,
                    &product.other_price_estimate_min,
                )
                .map(u64::from),
            price_estimate_min_pln: Currency::Pln
                .extract_amount(
                    &product.native_price_estimate_min,
                    &product.other_price_estimate_min,
                )
                .map(u64::from),
            price_estimate_min_try: Currency::Try
                .extract_amount(
                    &product.native_price_estimate_min,
                    &product.other_price_estimate_min,
                )
                .map(u64::from),
            price_estimate_min_jpy: Currency::Jpy
                .extract_amount(
                    &product.native_price_estimate_min,
                    &product.other_price_estimate_min,
                )
                .map(u64::from),
            price_estimate_min_czk: Currency::Czk
                .extract_amount(
                    &product.native_price_estimate_min,
                    &product.other_price_estimate_min,
                )
                .map(u64::from),
            price_estimate_min_rub: Currency::Rub
                .extract_amount(
                    &product.native_price_estimate_min,
                    &product.other_price_estimate_min,
                )
                .map(u64::from),
            price_estimate_min_aed: Currency::Aed
                .extract_amount(
                    &product.native_price_estimate_min,
                    &product.other_price_estimate_min,
                )
                .map(u64::from),
            price_estimate_min_sar: Currency::Sar
                .extract_amount(
                    &product.native_price_estimate_min,
                    &product.other_price_estimate_min,
                )
                .map(u64::from),
            price_estimate_min_hkd: Currency::Hkd
                .extract_amount(
                    &product.native_price_estimate_min,
                    &product.other_price_estimate_min,
                )
                .map(u64::from),
            price_estimate_min_sgd: Currency::Sgd
                .extract_amount(
                    &product.native_price_estimate_min,
                    &product.other_price_estimate_min,
                )
                .map(u64::from),
            price_estimate_min_chf: Currency::Chf
                .extract_amount(
                    &product.native_price_estimate_min,
                    &product.other_price_estimate_min,
                )
                .map(u64::from),
            price_estimate_max_eur: Currency::Eur
                .extract_amount(
                    &product.native_price_estimate_max,
                    &product.other_price_estimate_max,
                )
                .map(u64::from),
            price_estimate_max_usd: Currency::Usd
                .extract_amount(
                    &product.native_price_estimate_max,
                    &product.other_price_estimate_max,
                )
                .map(u64::from),
            price_estimate_max_gbp: Currency::Gbp
                .extract_amount(
                    &product.native_price_estimate_max,
                    &product.other_price_estimate_max,
                )
                .map(u64::from),
            price_estimate_max_aud: Currency::Aud
                .extract_amount(
                    &product.native_price_estimate_max,
                    &product.other_price_estimate_max,
                )
                .map(u64::from),
            price_estimate_max_cad: Currency::Cad
                .extract_amount(
                    &product.native_price_estimate_max,
                    &product.other_price_estimate_max,
                )
                .map(u64::from),
            price_estimate_max_nzd: Currency::Nzd
                .extract_amount(
                    &product.native_price_estimate_max,
                    &product.other_price_estimate_max,
                )
                .map(u64::from),
            price_estimate_max_cny: Currency::Cny
                .extract_amount(
                    &product.native_price_estimate_max,
                    &product.other_price_estimate_max,
                )
                .map(u64::from),
            price_estimate_max_brl: Currency::Brl
                .extract_amount(
                    &product.native_price_estimate_max,
                    &product.other_price_estimate_max,
                )
                .map(u64::from),
            price_estimate_max_pln: Currency::Pln
                .extract_amount(
                    &product.native_price_estimate_max,
                    &product.other_price_estimate_max,
                )
                .map(u64::from),
            price_estimate_max_try: Currency::Try
                .extract_amount(
                    &product.native_price_estimate_max,
                    &product.other_price_estimate_max,
                )
                .map(u64::from),
            price_estimate_max_jpy: Currency::Jpy
                .extract_amount(
                    &product.native_price_estimate_max,
                    &product.other_price_estimate_max,
                )
                .map(u64::from),
            price_estimate_max_czk: Currency::Czk
                .extract_amount(
                    &product.native_price_estimate_max,
                    &product.other_price_estimate_max,
                )
                .map(u64::from),
            price_estimate_max_rub: Currency::Rub
                .extract_amount(
                    &product.native_price_estimate_max,
                    &product.other_price_estimate_max,
                )
                .map(u64::from),
            price_estimate_max_aed: Currency::Aed
                .extract_amount(
                    &product.native_price_estimate_max,
                    &product.other_price_estimate_max,
                )
                .map(u64::from),
            price_estimate_max_sar: Currency::Sar
                .extract_amount(
                    &product.native_price_estimate_max,
                    &product.other_price_estimate_max,
                )
                .map(u64::from),
            price_estimate_max_hkd: Currency::Hkd
                .extract_amount(
                    &product.native_price_estimate_max,
                    &product.other_price_estimate_max,
                )
                .map(u64::from),
            price_estimate_max_sgd: Currency::Sgd
                .extract_amount(
                    &product.native_price_estimate_max,
                    &product.other_price_estimate_max,
                )
                .map(u64::from),
            price_estimate_max_chf: Currency::Chf
                .extract_amount(
                    &product.native_price_estimate_max,
                    &product.other_price_estimate_max,
                )
                .map(u64::from),
            state: product.state.into(),
            url: product.url,
            images: product
                .images
                .into_iter()
                .map(ProductImageDocument::from)
                .collect(),
            embedding: product.embedding,
            origin_year_min,
            origin_year,
            origin_year_max,
            authenticity: product.authenticity.into(),
            condition: product.condition.into(),
            provenance: product.provenance.into(),
            restoration: product.restoration.into(),
            auction_start: product.auction_start,
            auction_end: product.auction_end,
            created: product.created,
            updated: product.updated,
        }
    }
}

impl From<ProductDocument> for Product {
    fn from(product_document: ProductDocument) -> Self {
        let mut category_name = HashMap::with_capacity(Language::COUNT);
        if let Some(category_en) = product_document.category_name_en {
            category_name.insert(Language::En, category_en.into());
        }
        if let Some(category_de) = product_document.category_name_de {
            category_name.insert(Language::De, category_de.into());
        }
        if let Some(category_fr) = product_document.category_name_fr {
            category_name.insert(Language::Fr, category_fr.into());
        }
        if let Some(category_es) = product_document.category_name_es {
            category_name.insert(Language::Es, category_es.into());
        }
        if let Some(category_it) = product_document.category_name_it {
            category_name.insert(Language::It, category_it.into());
        }
        let mut period_name = HashMap::with_capacity(Language::COUNT);
        if let Some(period_en) = product_document.period_name_en {
            period_name.insert(Language::En, period_en.into());
        }
        if let Some(period_de) = product_document.period_name_de {
            period_name.insert(Language::De, period_de.into());
        }
        if let Some(period_fr) = product_document.period_name_fr {
            period_name.insert(Language::Fr, period_fr.into());
        }
        if let Some(period_es) = product_document.period_name_es {
            period_name.insert(Language::Es, period_es.into());
        }
        if let Some(period_it) = product_document.period_name_it {
            period_name.insert(Language::It, period_it.into());
        }

        let mut other_title = HashMap::with_capacity(Language::COUNT);
        if let Some(title_en) = product_document.title_en {
            other_title.insert(Language::En, title_en.into());
        }
        if let Some(title_de) = product_document.title_de {
            other_title.insert(Language::De, title_de.into());
        }
        if let Some(title_fr) = product_document.title_fr {
            other_title.insert(Language::Fr, title_fr.into());
        }
        if let Some(title_es) = product_document.title_es {
            other_title.insert(Language::Es, title_es.into());
        }
        if let Some(title_it) = product_document.title_it {
            other_title.insert(Language::It, title_it.into());
        }

        let mut other_price = HashMap::with_capacity(Currency::COUNT);
        if let Some(price_eur) = product_document.price_eur {
            other_price.insert(Currency::Eur, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_gbp {
            other_price.insert(Currency::Gbp, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_usd {
            other_price.insert(Currency::Usd, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_aud {
            other_price.insert(Currency::Aud, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_cad {
            other_price.insert(Currency::Cad, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_nzd {
            other_price.insert(Currency::Nzd, price_eur.into());
        }
        if let Some(val) = product_document.price_cny {
            other_price.insert(Currency::Cny, val.into());
        }
        if let Some(val) = product_document.price_brl {
            other_price.insert(Currency::Brl, val.into());
        }
        if let Some(val) = product_document.price_pln {
            other_price.insert(Currency::Pln, val.into());
        }
        if let Some(val) = product_document.price_try {
            other_price.insert(Currency::Try, val.into());
        }
        if let Some(val) = product_document.price_jpy {
            other_price.insert(Currency::Jpy, val.into());
        }
        if let Some(val) = product_document.price_czk {
            other_price.insert(Currency::Czk, val.into());
        }
        if let Some(val) = product_document.price_rub {
            other_price.insert(Currency::Rub, val.into());
        }
        if let Some(val) = product_document.price_aed {
            other_price.insert(Currency::Aed, val.into());
        }
        if let Some(val) = product_document.price_sar {
            other_price.insert(Currency::Sar, val.into());
        }
        if let Some(val) = product_document.price_hkd {
            other_price.insert(Currency::Hkd, val.into());
        }
        if let Some(val) = product_document.price_sgd {
            other_price.insert(Currency::Sgd, val.into());
        }
        if let Some(val) = product_document.price_chf {
            other_price.insert(Currency::Chf, val.into());
        }

        let mut other_price_estimate_min = HashMap::with_capacity(Currency::COUNT);
        if let Some(price_eur) = product_document.price_estimate_min_eur {
            other_price_estimate_min.insert(Currency::Eur, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_estimate_min_gbp {
            other_price_estimate_min.insert(Currency::Gbp, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_estimate_min_usd {
            other_price_estimate_min.insert(Currency::Usd, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_estimate_min_aud {
            other_price_estimate_min.insert(Currency::Aud, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_estimate_min_cad {
            other_price_estimate_min.insert(Currency::Cad, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_estimate_min_nzd {
            other_price_estimate_min.insert(Currency::Nzd, price_eur.into());
        }
        if let Some(val) = product_document.price_estimate_min_cny {
            other_price_estimate_min.insert(Currency::Cny, val.into());
        }
        if let Some(val) = product_document.price_estimate_min_brl {
            other_price_estimate_min.insert(Currency::Brl, val.into());
        }
        if let Some(val) = product_document.price_estimate_min_pln {
            other_price_estimate_min.insert(Currency::Pln, val.into());
        }
        if let Some(val) = product_document.price_estimate_min_try {
            other_price_estimate_min.insert(Currency::Try, val.into());
        }
        if let Some(val) = product_document.price_estimate_min_jpy {
            other_price_estimate_min.insert(Currency::Jpy, val.into());
        }
        if let Some(val) = product_document.price_estimate_min_czk {
            other_price_estimate_min.insert(Currency::Czk, val.into());
        }
        if let Some(val) = product_document.price_estimate_min_rub {
            other_price_estimate_min.insert(Currency::Rub, val.into());
        }
        if let Some(val) = product_document.price_estimate_min_aed {
            other_price_estimate_min.insert(Currency::Aed, val.into());
        }
        if let Some(val) = product_document.price_estimate_min_sar {
            other_price_estimate_min.insert(Currency::Sar, val.into());
        }
        if let Some(val) = product_document.price_estimate_min_hkd {
            other_price_estimate_min.insert(Currency::Hkd, val.into());
        }
        if let Some(val) = product_document.price_estimate_min_sgd {
            other_price_estimate_min.insert(Currency::Sgd, val.into());
        }
        if let Some(val) = product_document.price_estimate_min_chf {
            other_price_estimate_min.insert(Currency::Chf, val.into());
        }

        let mut other_price_estimate_max = HashMap::with_capacity(Currency::COUNT);
        if let Some(price_eur) = product_document.price_estimate_max_eur {
            other_price_estimate_max.insert(Currency::Eur, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_estimate_max_gbp {
            other_price_estimate_max.insert(Currency::Gbp, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_estimate_max_usd {
            other_price_estimate_max.insert(Currency::Usd, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_estimate_max_aud {
            other_price_estimate_max.insert(Currency::Aud, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_estimate_max_cad {
            other_price_estimate_max.insert(Currency::Cad, price_eur.into());
        }
        if let Some(price_eur) = product_document.price_estimate_max_nzd {
            other_price_estimate_max.insert(Currency::Nzd, price_eur.into());
        }
        if let Some(val) = product_document.price_estimate_max_cny {
            other_price_estimate_max.insert(Currency::Cny, val.into());
        }
        if let Some(val) = product_document.price_estimate_max_brl {
            other_price_estimate_max.insert(Currency::Brl, val.into());
        }
        if let Some(val) = product_document.price_estimate_max_pln {
            other_price_estimate_max.insert(Currency::Pln, val.into());
        }
        if let Some(val) = product_document.price_estimate_max_try {
            other_price_estimate_max.insert(Currency::Try, val.into());
        }
        if let Some(val) = product_document.price_estimate_max_jpy {
            other_price_estimate_max.insert(Currency::Jpy, val.into());
        }
        if let Some(val) = product_document.price_estimate_max_czk {
            other_price_estimate_max.insert(Currency::Czk, val.into());
        }
        if let Some(val) = product_document.price_estimate_max_rub {
            other_price_estimate_max.insert(Currency::Rub, val.into());
        }
        if let Some(val) = product_document.price_estimate_max_aed {
            other_price_estimate_max.insert(Currency::Aed, val.into());
        }
        if let Some(val) = product_document.price_estimate_max_sar {
            other_price_estimate_max.insert(Currency::Sar, val.into());
        }
        if let Some(val) = product_document.price_estimate_max_hkd {
            other_price_estimate_max.insert(Currency::Hkd, val.into());
        }
        if let Some(val) = product_document.price_estimate_max_sgd {
            other_price_estimate_max.insert(Currency::Sgd, val.into());
        }
        if let Some(val) = product_document.price_estimate_max_chf {
            other_price_estimate_max.insert(Currency::Chf, val.into());
        }

        Product {
            product_id: product_document.product_id,
            product_slug_id: product_document.product_slug_id,
            shop_slug_id: product_document.shop_slug_id,
            seller_slug_id: product_document.seller_slug_id,
            event_id: product_document.event_id,
            shop_id: product_document.shop_id,
            seller_id: product_document.seller_id,
            shops_product_id: product_document.shops_product_id,
            shop_name: product_document.shop_name.into(),
            seller_name: product_document.seller_name.into(),
            shop_type: product_document.shop_type.into(),
            structured_address: structured_address_from_document(
                product_document.structured_address_addressline,
                product_document.structured_address_addressline_extra,
                product_document.structured_address_locality,
                product_document.structured_address_region,
                product_document.structured_address_postal_code,
                product_document.structured_address_country,
            ),
            geo_address: geo_address_from_document(product_document.geo_address.as_deref()),
            category_id: product_document.category_id,
            category_name,
            period_id: product_document.period_id,
            period_name,
            native_title: Localized::new(
                product_document.title_native.language.into(),
                product_document.title_native.text.into(),
            ),
            other_title,
            native_description: None,
            native_price: None,
            other_price,
            native_price_estimate_min: None,
            other_price_estimate_min,
            native_price_estimate_max: None,
            other_price_estimate_max,
            state: product_document.state.into(),
            url: append_utm_params(product_document.url),
            images: product_document
                .images
                .into_iter()
                .map(ProductImage::from)
                .collect(),
            embedding: product_document.embedding,
            origin_year: match product_document.origin_year {
                Some(exact_year) => Some(OriginYear::ExactYear(exact_year)),
                None => match (
                    product_document.origin_year_min,
                    product_document.origin_year_max,
                ) {
                    (None, None) => None,
                    (min, max) => Some(OriginYear::EstimatedRange(YearRange { min, max })),
                },
            },
            authenticity: product_document.authenticity.into(),
            condition: product_document.condition.into(),
            provenance: product_document.provenance.into(),
            restoration: product_document.restoration.into(),
            auction_start: product_document.auction_start,
            auction_end: product_document.auction_end,
            created: product_document.created,
            updated: product_document.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use crate::core::description::Description;
    use crate::core::title::Title;
    use common::price::domain::MonetaryAmount;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ProductDocument {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let state: ProductStateDocument = config.fake_with_rng(rng);
            let origin_year_min = fake::rand::random_range(1807..=1815).into();
            let origin_year_max = fake::rand::random_range(1815..=1819).into();
            let origin_year = if origin_year_min == origin_year_max {
                Some(origin_year_min)
            } else {
                None
            };
            let title_native = TextDocument {
                text: config.fake_with_rng::<Title, _>(rng).to_string(),
                language: config.fake_with_rng(rng),
            };
            let shop_name: String = config.fake_with_rng(rng);
            let seller_name: String = config.fake_with_rng(rng);
            ProductDocument {
                product_id: config.fake_with_rng(rng),
                product_slug_id: SlugId::from(&title_native.text),
                shop_slug_id: SlugId::from(&shop_name),
                seller_slug_id: SlugId::from(&seller_name),
                event_id: config.fake_with_rng(rng),
                shop_id: config.fake_with_rng(rng),
                seller_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                category_id: config.fake_with_rng(rng),
                period_id: config.fake_with_rng(rng),
                category_name_de: config.fake_with_rng(rng),
                category_name_en: config.fake_with_rng(rng),
                category_name_fr: config.fake_with_rng(rng),
                category_name_es: config.fake_with_rng(rng),
                category_name_it: config.fake_with_rng(rng),
                period_name_de: config.fake_with_rng(rng),
                period_name_en: config.fake_with_rng(rng),
                period_name_fr: config.fake_with_rng(rng),
                period_name_es: config.fake_with_rng(rng),
                period_name_it: config.fake_with_rng(rng),
                shop_name,
                seller_name,
                shop_type: config.fake_with_rng(rng),
                structured_address_addressline: config.fake_with_rng(rng),
                structured_address_addressline_extra: config.fake_with_rng(rng),
                structured_address_locality: config.fake_with_rng(rng),
                structured_address_region: config.fake_with_rng(rng),
                structured_address_postal_code: config.fake_with_rng(rng),
                structured_address_country: None,
                structured_address_continent: None,
                geo_address: None,
                title_native,
                title_de: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                title_en: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                title_fr: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                title_es: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                title_it: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                price_eur: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_usd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_gbp: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_aud: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_cad: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_nzd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_cny: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_brl: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_pln: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_try: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_jpy: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_czk: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_rub: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_aed: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_sar: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_hkd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_sgd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_chf: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_eur: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_usd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_gbp: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_aud: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_cad: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_nzd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_cny: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_brl: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_pln: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_try: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_jpy: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_czk: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_rub: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_aed: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_sar: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_hkd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_sgd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_min_chf: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_eur: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_usd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_gbp: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_aud: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_cad: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_nzd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_cny: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_brl: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_pln: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_try: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_jpy: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_czk: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_rub: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_aed: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_sar: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_hkd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_sgd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_estimate_max_chf: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                state,
                url: Url::parse(&format!(
                    "https://foo.bar/item/{}",
                    config.fake_with_rng::<u16, _>(rng)
                ))
                .unwrap(),
                images: config.fake_with_rng(rng),
                embedding: None,
                origin_year_min: Some(origin_year_min),
                origin_year,
                origin_year_max: Some(origin_year_max),
                authenticity: config.fake_with_rng(rng),
                condition: config.fake_with_rng(rng),
                provenance: config.fake_with_rng(rng),
                restoration: config.fake_with_rng(rng),
                auction_start: if config.fake_with_rng(rng) {
                    Some(OffsetDateTime::now_utc())
                } else {
                    None
                },
                auction_end: if config.fake_with_rng(rng) {
                    Some(OffsetDateTime::now_utc())
                } else {
                    None
                },
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::opensearch::product_document::ProductDocument;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_product_document() {
            let _ = Faker.fake::<ProductDocument>();
        }
    }
}
