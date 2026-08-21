use crate::core::product::Product;
use crate::core::product_image::ProductImage;
use crate::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use crate::dynamodb::product_image_record::ProductImageRecord;
use crate::dynamodb::product_state_record::ProductStateRecord;
use common::actor::record::ActorRecord;
use common::currency::domain::Currency;
use common::error::mapping_error::PersistenceMappingError;
use common::error::missing_field::MissingPersistenceField;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::language::domain::Language;
use common::language::record::TextRecord;
use common::localized::Localized;
use common::price::domain::Price;
use common::price::record::PriceRecord;
use common::product_id::{ProductId, ProductKey};
use common::product_lifecycle::record::ProductLifecycleRecord;
use common::product_slug_id::ProductSlugId;
use common::seller_slug_id::SellerSlugId;
use common::shop_id::ShopId;
use common::shop_slug_id::ShopSlugId;
use common::shops_product_id::ShopsProductId;
use field::field;
use geo::dynamodb::{geo_address_from_record, structured_address_from_record};
use indexmap::IndexSet;
use isocountry::CountryCode;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use shop::dynamodb::shop_type_record::ShopTypeRecord;
use std::collections::HashMap;
use strum::EnumCount;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct ProductRecord {
    pub pk: String,
    pub sk: String,
    pub gsi2_pk: String,
    pub gsi2_sk: String,

    pub product_id: ProductId,
    pub product_slug_id: ProductSlugId,
    pub shop_slug_id: ShopSlugId,
    pub seller_slug_id: SellerSlugId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: String,
    pub seller_name: String,
    pub shop_type: ShopTypeRecord,

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
    pub geo_address_lat: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub geo_address_lon: Option<f64>,

    pub title_native: TextRecord,
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
    pub description_native: Option<TextRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_native: Option<PriceRecord>,
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
    pub price_estimate_min_native: Option<PriceRecord>,
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
    pub price_estimate_max_native: Option<PriceRecord>,
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

    pub state: ProductStateRecord,
    pub lifecycle: ProductLifecycleRecord,
    pub url: Url,
    pub view_url: Url,
    #[serde(skip_serializing_if = "IndexSet::is_empty", default)]
    pub images: IndexSet<ProductImageRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub embedding: Option<Vec<f32>>,

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

    pub created_by: ActorRecord,
    pub updated_by: ActorRecord,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

pub fn mk_pk(shop_id: &ShopId, shops_product_id: &ShopsProductId) -> String {
    format!(
        "product#{}",
        super::product_key::encode(shop_id, shops_product_id)
    )
}

pub fn mk_sk() -> &'static str {
    "product#materialized"
}

pub fn mk_gsi2_pk(shop_slug_id: &ShopSlugId, product_slug_id: &ProductSlugId) -> String {
    format!("shop_slug_id#{shop_slug_id}#product_slug_id#{product_slug_id}")
}

pub fn mk_gsi2_sk() -> &'static str {
    "product#lookup#shop_id#shops_product_id"
}

impl HasKey for ProductRecord {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        ProductKey {
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id.clone(),
        }
    }
}

impl From<ProductRecord> for Product {
    fn from(record: ProductRecord) -> Self {
        let mut other_title = HashMap::with_capacity(Language::COUNT);
        if let Some(title_en) = record.title_en {
            other_title.insert(Language::En, title_en.into());
        }
        if let Some(title_de) = record.title_de {
            other_title.insert(Language::De, title_de.into());
        }
        if let Some(title_fr) = record.title_fr {
            other_title.insert(Language::Fr, title_fr.into());
        }
        if let Some(title_es) = record.title_es {
            other_title.insert(Language::Es, title_es.into());
        }
        if let Some(title_it) = record.title_it {
            other_title.insert(Language::It, title_it.into());
        }

        let mut other_price = HashMap::with_capacity(Currency::COUNT);
        if let Some(price_eur) = record.price_eur {
            other_price.insert(Currency::Eur, price_eur.into());
        }
        if let Some(price_eur) = record.price_gbp {
            other_price.insert(Currency::Gbp, price_eur.into());
        }
        if let Some(price_eur) = record.price_usd {
            other_price.insert(Currency::Usd, price_eur.into());
        }
        if let Some(price_eur) = record.price_aud {
            other_price.insert(Currency::Aud, price_eur.into());
        }
        if let Some(price_eur) = record.price_cad {
            other_price.insert(Currency::Cad, price_eur.into());
        }
        if let Some(price_eur) = record.price_nzd {
            other_price.insert(Currency::Nzd, price_eur.into());
        }
        if let Some(val) = record.price_cny {
            other_price.insert(Currency::Cny, val.into());
        }
        if let Some(val) = record.price_brl {
            other_price.insert(Currency::Brl, val.into());
        }
        if let Some(val) = record.price_pln {
            other_price.insert(Currency::Pln, val.into());
        }
        if let Some(val) = record.price_try {
            other_price.insert(Currency::Try, val.into());
        }
        if let Some(val) = record.price_jpy {
            other_price.insert(Currency::Jpy, val.into());
        }
        if let Some(val) = record.price_czk {
            other_price.insert(Currency::Czk, val.into());
        }
        if let Some(val) = record.price_rub {
            other_price.insert(Currency::Rub, val.into());
        }
        if let Some(val) = record.price_aed {
            other_price.insert(Currency::Aed, val.into());
        }
        if let Some(val) = record.price_sar {
            other_price.insert(Currency::Sar, val.into());
        }
        if let Some(val) = record.price_hkd {
            other_price.insert(Currency::Hkd, val.into());
        }
        if let Some(val) = record.price_sgd {
            other_price.insert(Currency::Sgd, val.into());
        }
        if let Some(val) = record.price_chf {
            other_price.insert(Currency::Chf, val.into());
        }

        let mut other_price_estimate_min = HashMap::with_capacity(Currency::COUNT);
        if let Some(price_eur) = record.price_estimate_min_eur {
            other_price_estimate_min.insert(Currency::Eur, price_eur.into());
        }
        if let Some(price_eur) = record.price_estimate_min_gbp {
            other_price_estimate_min.insert(Currency::Gbp, price_eur.into());
        }
        if let Some(price_eur) = record.price_estimate_min_usd {
            other_price_estimate_min.insert(Currency::Usd, price_eur.into());
        }
        if let Some(price_eur) = record.price_estimate_min_aud {
            other_price_estimate_min.insert(Currency::Aud, price_eur.into());
        }
        if let Some(price_eur) = record.price_estimate_min_cad {
            other_price_estimate_min.insert(Currency::Cad, price_eur.into());
        }
        if let Some(price_eur) = record.price_estimate_min_nzd {
            other_price_estimate_min.insert(Currency::Nzd, price_eur.into());
        }
        if let Some(val) = record.price_estimate_min_cny {
            other_price_estimate_min.insert(Currency::Cny, val.into());
        }
        if let Some(val) = record.price_estimate_min_brl {
            other_price_estimate_min.insert(Currency::Brl, val.into());
        }
        if let Some(val) = record.price_estimate_min_pln {
            other_price_estimate_min.insert(Currency::Pln, val.into());
        }
        if let Some(val) = record.price_estimate_min_try {
            other_price_estimate_min.insert(Currency::Try, val.into());
        }
        if let Some(val) = record.price_estimate_min_jpy {
            other_price_estimate_min.insert(Currency::Jpy, val.into());
        }
        if let Some(val) = record.price_estimate_min_czk {
            other_price_estimate_min.insert(Currency::Czk, val.into());
        }
        if let Some(val) = record.price_estimate_min_rub {
            other_price_estimate_min.insert(Currency::Rub, val.into());
        }
        if let Some(val) = record.price_estimate_min_aed {
            other_price_estimate_min.insert(Currency::Aed, val.into());
        }
        if let Some(val) = record.price_estimate_min_sar {
            other_price_estimate_min.insert(Currency::Sar, val.into());
        }
        if let Some(val) = record.price_estimate_min_hkd {
            other_price_estimate_min.insert(Currency::Hkd, val.into());
        }
        if let Some(val) = record.price_estimate_min_sgd {
            other_price_estimate_min.insert(Currency::Sgd, val.into());
        }
        if let Some(val) = record.price_estimate_min_chf {
            other_price_estimate_min.insert(Currency::Chf, val.into());
        }

        let mut other_price_estimate_max = HashMap::with_capacity(Currency::COUNT);
        if let Some(price_eur) = record.price_estimate_max_eur {
            other_price_estimate_max.insert(Currency::Eur, price_eur.into());
        }
        if let Some(price_eur) = record.price_estimate_max_gbp {
            other_price_estimate_max.insert(Currency::Gbp, price_eur.into());
        }
        if let Some(price_eur) = record.price_estimate_max_usd {
            other_price_estimate_max.insert(Currency::Usd, price_eur.into());
        }
        if let Some(price_eur) = record.price_estimate_max_aud {
            other_price_estimate_max.insert(Currency::Aud, price_eur.into());
        }
        if let Some(price_eur) = record.price_estimate_max_cad {
            other_price_estimate_max.insert(Currency::Cad, price_eur.into());
        }
        if let Some(price_eur) = record.price_estimate_max_nzd {
            other_price_estimate_max.insert(Currency::Nzd, price_eur.into());
        }
        if let Some(val) = record.price_estimate_max_cny {
            other_price_estimate_max.insert(Currency::Cny, val.into());
        }
        if let Some(val) = record.price_estimate_max_brl {
            other_price_estimate_max.insert(Currency::Brl, val.into());
        }
        if let Some(val) = record.price_estimate_max_pln {
            other_price_estimate_max.insert(Currency::Pln, val.into());
        }
        if let Some(val) = record.price_estimate_max_try {
            other_price_estimate_max.insert(Currency::Try, val.into());
        }
        if let Some(val) = record.price_estimate_max_jpy {
            other_price_estimate_max.insert(Currency::Jpy, val.into());
        }
        if let Some(val) = record.price_estimate_max_czk {
            other_price_estimate_max.insert(Currency::Czk, val.into());
        }
        if let Some(val) = record.price_estimate_max_rub {
            other_price_estimate_max.insert(Currency::Rub, val.into());
        }
        if let Some(val) = record.price_estimate_max_aed {
            other_price_estimate_max.insert(Currency::Aed, val.into());
        }
        if let Some(val) = record.price_estimate_max_sar {
            other_price_estimate_max.insert(Currency::Sar, val.into());
        }
        if let Some(val) = record.price_estimate_max_hkd {
            other_price_estimate_max.insert(Currency::Hkd, val.into());
        }
        if let Some(val) = record.price_estimate_max_sgd {
            other_price_estimate_max.insert(Currency::Sgd, val.into());
        }
        if let Some(val) = record.price_estimate_max_chf {
            other_price_estimate_max.insert(Currency::Chf, val.into());
        }

        Product {
            product_id: record.product_id,
            product_slug_id: record.product_slug_id,
            shop_slug_id: record.shop_slug_id,
            seller_slug_id: record.seller_slug_id,
            event_id: record.event_id,
            shop_id: record.shop_id,
            seller_id: record.seller_id,
            shops_product_id: record.shops_product_id,
            shop_name: record.shop_name.into(),
            seller_name: record.seller_name.into(),
            shop_type: record.shop_type.into(),
            structured_address: structured_address_from_record(
                record.structured_address_addressline,
                record.structured_address_addressline_extra,
                record.structured_address_locality,
                record.structured_address_region,
                record.structured_address_postal_code,
                record.structured_address_country,
            ),
            geo_address: geo_address_from_record(record.geo_address_lat, record.geo_address_lon),
            native_title: Localized::new(
                record.title_native.language.into(),
                record.title_native.text.into(),
            ),
            other_title,
            native_description: record.description_native.map(|text_record| {
                Localized::new(text_record.language.into(), text_record.text.into())
            }),
            native_price: record.price_native.map(Price::from),
            other_price,
            native_price_estimate_min: record.price_estimate_min_native.map(Price::from),
            other_price_estimate_min,
            native_price_estimate_max: record.price_estimate_max_native.map(Price::from),
            other_price_estimate_max,
            state: record.state.into(),
            lifecycle: record.lifecycle.into(),
            url: record.url.clone(),
            view_url: record.view_url,
            images: record.images.into_iter().map(ProductImage::from).collect(),
            embedding: record.embedding,
            auction_start: record.auction_start,
            auction_end: record.auction_end,
            created_by: record.created_by.into(),
            updated_by: record.updated_by.into(),
            created: record.created,
            updated: record.updated,
        }
    }
}

impl TryFrom<ProductDomainEventRecord> for ProductRecord {
    type Error = PersistenceMappingError;

    fn try_from(event_record: ProductDomainEventRecord) -> Result<Self, Self::Error> {
        let product_slug_id = event_record.product_slug_id.ok_or_else(|| {
            MissingPersistenceField::new(field!(product_slug_id@ProductDomainEventRecord))
        })?;
        let shop_slug_id = event_record.shop_slug_id.ok_or_else(|| {
            MissingPersistenceField::new(field!(shop_slug_id@ProductDomainEventRecord))
        })?;
        let seller_slug_id = event_record.seller_slug_id.ok_or_else(|| {
            MissingPersistenceField::new(field!(seller_slug_id@ProductDomainEventRecord))
        })?;
        let record = ProductRecord {
            pk: event_record.pk,
            sk: mk_sk().to_string(),
            gsi2_pk: mk_gsi2_pk(&shop_slug_id, &product_slug_id),
            gsi2_sk: mk_gsi2_sk().to_owned(),
            product_id: event_record.product_id,
            product_slug_id,
            shop_slug_id,
            seller_slug_id,
            event_id: event_record.event_id,
            shop_id: event_record.shop_id,
            seller_id: event_record.seller_id,
            shops_product_id: event_record.shops_product_id,
            shop_name: event_record.shop_name.ok_or_else(|| {
                MissingPersistenceField::new(field!(shop_name@ProductDomainEventRecord))
            })?,
            seller_name: event_record.seller_name.ok_or_else(|| {
                MissingPersistenceField::new(field!(seller_name@ProductDomainEventRecord))
            })?,
            shop_type: event_record.shop_type.ok_or_else(|| {
                MissingPersistenceField::new(field!(shop_type@ProductDomainEventRecord))
            })?,
            structured_address_addressline: event_record.structured_address_addressline,
            structured_address_addressline_extra: event_record.structured_address_addressline_extra,
            structured_address_locality: event_record.structured_address_locality,
            structured_address_region: event_record.structured_address_region,
            structured_address_postal_code: event_record.structured_address_postal_code,
            structured_address_country: event_record.structured_address_country,
            geo_address_lat: event_record.geo_address_lat,
            geo_address_lon: event_record.geo_address_lon,
            title_native: event_record.title_native.ok_or_else(|| {
                MissingPersistenceField::new(field!(title_native@ProductDomainEventRecord))
            })?,
            title_de: event_record.title_de,
            title_en: event_record.title_en,
            title_fr: event_record.title_fr,
            title_es: event_record.title_es,
            title_it: event_record.title_it,
            description_native: event_record.description_native,
            price_native: event_record.new_price_native,
            price_eur: event_record.new_price_eur,
            price_usd: event_record.new_price_usd,
            price_gbp: event_record.new_price_gbp,
            price_aud: event_record.new_price_aud,
            price_cad: event_record.new_price_cad,
            price_nzd: event_record.new_price_nzd,
            price_cny: event_record.new_price_cny,
            price_brl: event_record.new_price_brl,
            price_pln: event_record.new_price_pln,
            price_try: event_record.new_price_try,
            price_jpy: event_record.new_price_jpy,
            price_czk: event_record.new_price_czk,
            price_rub: event_record.new_price_rub,
            price_aed: event_record.new_price_aed,
            price_sar: event_record.new_price_sar,
            price_hkd: event_record.new_price_hkd,
            price_sgd: event_record.new_price_sgd,
            price_chf: event_record.new_price_chf,
            price_estimate_min_native: event_record.new_price_estimate_min_native,
            price_estimate_min_eur: event_record.new_price_estimate_min_eur,
            price_estimate_min_usd: event_record.new_price_estimate_min_usd,
            price_estimate_min_gbp: event_record.new_price_estimate_min_gbp,
            price_estimate_min_aud: event_record.new_price_estimate_min_aud,
            price_estimate_min_cad: event_record.new_price_estimate_min_cad,
            price_estimate_min_nzd: event_record.new_price_estimate_min_nzd,
            price_estimate_min_cny: event_record.new_price_estimate_min_cny,
            price_estimate_min_brl: event_record.new_price_estimate_min_brl,
            price_estimate_min_pln: event_record.new_price_estimate_min_pln,
            price_estimate_min_try: event_record.new_price_estimate_min_try,
            price_estimate_min_jpy: event_record.new_price_estimate_min_jpy,
            price_estimate_min_czk: event_record.new_price_estimate_min_czk,
            price_estimate_min_rub: event_record.new_price_estimate_min_rub,
            price_estimate_min_aed: event_record.new_price_estimate_min_aed,
            price_estimate_min_sar: event_record.new_price_estimate_min_sar,
            price_estimate_min_hkd: event_record.new_price_estimate_min_hkd,
            price_estimate_min_sgd: event_record.new_price_estimate_min_sgd,
            price_estimate_min_chf: event_record.new_price_estimate_min_chf,
            price_estimate_max_native: event_record.new_price_estimate_max_native,
            price_estimate_max_eur: event_record.new_price_estimate_max_eur,
            price_estimate_max_usd: event_record.new_price_estimate_max_usd,
            price_estimate_max_gbp: event_record.new_price_estimate_max_gbp,
            price_estimate_max_aud: event_record.new_price_estimate_max_aud,
            price_estimate_max_cad: event_record.new_price_estimate_max_cad,
            price_estimate_max_nzd: event_record.new_price_estimate_max_nzd,
            price_estimate_max_cny: event_record.new_price_estimate_max_cny,
            price_estimate_max_brl: event_record.new_price_estimate_max_brl,
            price_estimate_max_pln: event_record.new_price_estimate_max_pln,
            price_estimate_max_try: event_record.new_price_estimate_max_try,
            price_estimate_max_jpy: event_record.new_price_estimate_max_jpy,
            price_estimate_max_czk: event_record.new_price_estimate_max_czk,
            price_estimate_max_rub: event_record.new_price_estimate_max_rub,
            price_estimate_max_aed: event_record.new_price_estimate_max_aed,
            price_estimate_max_sar: event_record.new_price_estimate_max_sar,
            price_estimate_max_hkd: event_record.new_price_estimate_max_hkd,
            price_estimate_max_sgd: event_record.new_price_estimate_max_sgd,
            price_estimate_max_chf: event_record.new_price_estimate_max_chf,
            state: event_record.new_state.ok_or_else(|| {
                MissingPersistenceField::new(field!(new_state@ProductDomainEventRecord))
            })?,
            lifecycle: ProductLifecycleRecord::Active,
            url: event_record.url.ok_or_else(|| {
                MissingPersistenceField::new(field!(url@ProductDomainEventRecord))
            })?,
            view_url: event_record.view_url.ok_or_else(|| {
                MissingPersistenceField::new(field!(view_url@ProductDomainEventRecord))
            })?,
            images: event_record.images.unwrap_or_default(),
            embedding: None,
            auction_start: event_record.auction_start,
            auction_end: event_record.auction_end,
            created_by: ActorRecord::System,
            updated_by: ActorRecord::System,
            created: event_record.timestamp,
            updated: event_record.timestamp,
        };

        Ok(record)
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use crate::core::description::Description;
    use crate::core::title::Title;
    use common::price::domain::MonetaryAmount;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ProductRecord {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let now = OffsetDateTime::now_utc();
            let shop_id: ShopId = config.fake_with_rng(rng);
            let shops_product_id: ShopsProductId = config.fake_with_rng(rng);
            let price_native: Option<PriceRecord> =
                Some(config.fake_with_rng::<Price, _>(rng).into());
            let state: ProductStateRecord = config.fake_with_rng(rng);

            let title_native = TextRecord::new(
                config.fake_with_rng::<Title, _>(rng).to_string(),
                config.fake_with_rng(rng),
            );
            let shop_name = config.fake_with_rng(rng);
            let seller_name = config.fake_with_rng(rng);
            let shop_slug_id = ShopSlugId::from(&shop_name);
            let seller_slug_id = SellerSlugId::from(&seller_name);
            let product_slug_id = ProductSlugId::from(&title_native.text);
            ProductRecord {
                pk: mk_pk(&shop_id, &shops_product_id),
                sk: mk_sk().to_string(),
                gsi2_pk: mk_gsi2_pk(&shop_slug_id, &product_slug_id),
                gsi2_sk: mk_gsi2_sk().to_owned(),
                product_id: config.fake_with_rng(rng),
                product_slug_id,
                shop_slug_id,
                seller_slug_id,
                event_id: config.fake_with_rng(rng),
                shop_id,
                seller_id: config.fake_with_rng(rng),
                shops_product_id: shops_product_id.clone(),
                shop_name,
                seller_name,
                shop_type: config.fake_with_rng(rng),
                structured_address_addressline: config.fake_with_rng(rng),
                structured_address_addressline_extra: config.fake_with_rng(rng),
                structured_address_locality: config.fake_with_rng(rng),
                structured_address_region: config.fake_with_rng(rng),
                structured_address_postal_code: config.fake_with_rng(rng),
                structured_address_country: None,
                geo_address_lat: config.fake_with_rng(rng),
                geo_address_lon: config.fake_with_rng(rng),
                title_native,
                title_de: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                title_en: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                title_fr: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                title_es: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                title_it: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                description_native: Some(TextRecord::new(
                    config.fake_with_rng::<Description, _>(rng).to_string(),
                    config.fake_with_rng(rng),
                )),
                price_native,
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
                price_estimate_min_native: Some(config.fake_with_rng::<Price, _>(rng).into()),
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
                price_estimate_max_native: Some(config.fake_with_rng::<Price, _>(rng).into()),
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
                lifecycle: ProductLifecycleRecord::Active,
                url: Url::parse(&format!(
                    "https://foo.bar/item/{}",
                    config.fake_with_rng::<u16, _>(rng)
                ))
                .unwrap(),
                view_url: Url::parse(&format!(
                    "https://foo.bar/item/{}?utm_source=aura_historia&utm_medium=referral",
                    config.fake_with_rng::<u16, _>(rng)
                ))
                .unwrap(),
                images: config
                    .fake_with_rng::<Vec<ProductImageRecord>, _>(rng)
                    .into_iter()
                    .collect(),
                embedding: if config.fake_with_rng(rng) {
                    Some(fake::vec![f32; 768])
                } else {
                    None
                },
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
                created_by: config.fake_with_rng(rng),
                updated_by: config.fake_with_rng(rng),
                created: now,
                updated: now,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::dynamodb::product_record::ProductRecord;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_get_product_record() {
            let _ = Faker.fake::<ProductRecord>();
        }
    }
}

#[cfg(all(test, feature = "test-data"))]
mod tests {
    use super::*;
    use crate::core::product::Product;
    use fake::{Fake, Faker};

    #[test]
    fn should_keep_raw_url_when_mapping_product_record_to_product() {
        let mut record = Faker.fake::<ProductRecord>();
        record.url = Url::parse("https://example-shop.com/item/42").unwrap();

        let product: Product = record.into();

        assert_eq!(product.url.as_str(), "https://example-shop.com/item/42");
    }

    #[test]
    fn should_use_stored_view_url_when_present() {
        let affiliate_url =
            Url::parse("https://prf.hn/click/camref:1110lF73C/pubref:aurahistoria/destination:https%3A%2F%2Fexample.com%2Fitem%2F42")
                .unwrap();
        let mut record = Faker.fake::<ProductRecord>();
        record.url = Url::parse("https://example.com/item/42").unwrap();
        record.view_url = affiliate_url.clone();

        let product: Product = record.into();

        assert_eq!(product.view_url, affiliate_url);
        assert_eq!(product.url.as_str(), "https://example.com/item/42");
    }
}
