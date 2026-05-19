use crate::core::product_event::ProductDomainEvent;
use crate::core::product_event::domain::{
    ProductAuctionTimeChangeDomainEventPayload, ProductCommonEventPayload,
    ProductCreatedDomainEventPayload, ProductDomainEventPayload,
    ProductEstimatePriceChangeDomainEventPayload, ProductImagesChangeDomainEventPayload,
    ProductPriceChangeDomainEventPayload, ProductStateChangeDomainEventPayload,
    ProductUrlChangeDomainEventPayload,
};
use crate::core::product_image::ProductImage;
use crate::dynamodb::product_event_type_record::domain::ProductDomainEventTypeRecord;
use crate::dynamodb::product_image_record::ProductImageRecord;
use crate::dynamodb::product_state_record::ProductStateRecord;
use common::currency::domain::Currency;
use common::error::missing_field::MissingPersistenceField;
use common::event::Event;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::language::domain::Language;
use common::language::record::TextRecord;
use common::localized::Localized;
use common::price::domain::Price;
use common::price::record::PriceRecord;
use common::product_id::{ProductId, ProductKey};
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shops_product_id::ShopsProductId;
use common::slug_id::SlugId;
use field::field;
use geo::dynamodb::{geo_address_from_record, structured_address_from_record};
use isocountry::CountryCode;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use shop::dynamodb::shop_type_record::ShopTypeRecord;
use std::collections::HashMap;
use strum::EnumCount;
use time::OffsetDateTime;
use url::Url;

use crate::dynamodb::utm::append_utm_params;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct ProductDomainEventRecord {
    pub pk: String,
    pub sk: String,
    pub product_id: ProductId,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub product_slug_id: Option<SlugId<6>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_slug_id: Option<SlugId<0>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub seller_slug_id: Option<SlugId<0>>,
    pub event_id: EventId,
    pub event_type: ProductDomainEventTypeRecord,
    pub event_type_schema_version: u8,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub seller_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_type: Option<ShopTypeRecord>,
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

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_native: Option<TextRecord>,
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
    pub new_price_native: Option<PriceRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_eur: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_usd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_gbp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_aud: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_cad: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_nzd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_cny: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_brl: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_pln: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_try: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_jpy: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_czk: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_rub: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_aed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_sar: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_hkd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_sgd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_chf: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_min_native: Option<PriceRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_min_eur: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_min_usd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_min_gbp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_min_aud: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_min_cad: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_min_nzd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_min_cny: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_min_brl: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_min_pln: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_min_try: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_min_jpy: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_min_czk: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_min_rub: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_min_aed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_min_sar: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_min_hkd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_min_sgd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_min_chf: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_max_native: Option<PriceRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_max_eur: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_max_usd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_max_gbp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_max_aud: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_max_cad: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_max_nzd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_max_cny: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_max_brl: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_max_pln: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_max_try: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_max_jpy: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_max_czk: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_max_rub: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_max_aed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_max_sar: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_max_hkd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_max_sgd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_estimate_max_chf: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_native: Option<PriceRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_eur: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_usd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_gbp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_aud: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_cad: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_nzd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_cny: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_brl: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_pln: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_try: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_jpy: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_czk: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_rub: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_aed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_sar: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_hkd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_sgd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_chf: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_state: Option<ProductStateRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_state: Option<ProductStateRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<Url>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub view_url: Option<Url>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub images: Option<Vec<ProductImageRecord>>,

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
    pub timestamp: OffsetDateTime,
}

pub fn mk_pk(shop_id: &ShopId, shops_product_id: &ShopsProductId) -> String {
    format!("product#shop_id#{shop_id}#shops_product_id#{shops_product_id}")
}

pub fn mk_sk(event_id: &EventId) -> String {
    format!("product#event#domain#{event_id}")
}

impl ProductDomainEventRecord {
    pub fn into_product_key(self) -> ProductKey {
        ProductKey::new(self.shop_id, self.shops_product_id)
    }
}

impl HasKey for ProductDomainEventRecord {
    type Key = ProductKey;

    fn key(&self) -> ProductKey {
        ProductKey {
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id.clone(),
        }
    }
}

impl From<ProductDomainEvent> for ProductDomainEventRecord {
    fn from(domain: ProductDomainEvent) -> Self {
        let shop_id = *domain.payload.shop_id();
        let shops_product_id = domain.payload.shops_product_id();
        let pk = mk_pk(&shop_id, shops_product_id);
        let sk = mk_sk(&domain.event_id);
        let product_id = domain.aggregate_id;
        let event_id = domain.event_id;
        let event_type: ProductDomainEventTypeRecord = (&domain.payload).into();
        let shops_product_id = shops_product_id.clone();

        match domain.payload {
            ProductDomainEventPayload::Created(payload) => {
                let (title_de, title_en, title_fr, title_es, title_it) =
                    match payload.native_title.localization {
                        Language::De => (
                            Some(payload.native_title.payload.to_string()),
                            None,
                            None,
                            None,
                            None,
                        ),
                        Language::En => (
                            None,
                            Some(payload.native_title.payload.to_string()),
                            None,
                            None,
                            None,
                        ),
                        Language::Fr => (
                            None,
                            None,
                            Some(payload.native_title.payload.to_string()),
                            None,
                            None,
                        ),
                        Language::Es => (
                            None,
                            None,
                            None,
                            Some(payload.native_title.payload.to_string()),
                            None,
                        ),
                        Language::It => (
                            None,
                            None,
                            None,
                            None,
                            Some(payload.native_title.payload.to_string()),
                        ),
                        // Ingestion-only languages have no dedicated title field
                        _ => (None, None, None, None, None),
                    };

                ProductDomainEventRecord {
                    pk,
                    sk,
                    product_id,
                    product_slug_id: Some(payload.product_slug_id),
                    shop_slug_id: Some(payload.shop_slug_id),
                    seller_slug_id: Some(payload.seller_slug_id),
                    event_id,
                    event_type,
                    event_type_schema_version: 0,
                    shop_id,
                    seller_id: payload.seller_id,
                    shops_product_id,
                    shop_name: Some(payload.shop_name.into()),
                    seller_name: Some(payload.seller_name.into()),
                    shop_type: Some(payload.shop_type.into()),
                    structured_address_addressline: payload
                        .structured_address
                        .as_ref()
                        .and_then(|a| a.addressline.clone()),
                    structured_address_addressline_extra: payload
                        .structured_address
                        .as_ref()
                        .and_then(|a| a.addressline_extra.clone()),
                    structured_address_locality: payload
                        .structured_address
                        .as_ref()
                        .and_then(|a| a.locality.clone()),
                    structured_address_region: payload
                        .structured_address
                        .as_ref()
                        .and_then(|a| a.region.clone()),
                    structured_address_postal_code: payload
                        .structured_address
                        .as_ref()
                        .and_then(|a| a.postal_code.clone()),
                    structured_address_country: payload
                        .structured_address
                        .as_ref()
                        .and_then(|a| a.country),
                    geo_address_lat: payload.geo_address.map(|address| address.lat),
                    geo_address_lon: payload.geo_address.map(|address| address.lon),
                    title_native: Some(payload.native_title.into()),
                    title_de,
                    title_en,
                    title_fr,
                    title_es,
                    title_it,
                    description_native: payload.native_description.map(TextRecord::from),
                    new_price_native: payload.native_price.map(PriceRecord::from),
                    new_price_eur: payload
                        .other_price
                        .get(&Currency::Eur)
                        .copied()
                        .map(u64::from),
                    new_price_usd: payload
                        .other_price
                        .get(&Currency::Usd)
                        .copied()
                        .map(u64::from),
                    new_price_gbp: payload
                        .other_price
                        .get(&Currency::Gbp)
                        .copied()
                        .map(u64::from),
                    new_price_aud: payload
                        .other_price
                        .get(&Currency::Aud)
                        .copied()
                        .map(u64::from),
                    new_price_cad: payload
                        .other_price
                        .get(&Currency::Cad)
                        .copied()
                        .map(u64::from),
                    new_price_nzd: payload
                        .other_price
                        .get(&Currency::Nzd)
                        .copied()
                        .map(u64::from),
                    new_price_cny: payload
                        .other_price
                        .get(&Currency::Cny)
                        .copied()
                        .map(u64::from),
                    new_price_brl: payload
                        .other_price
                        .get(&Currency::Brl)
                        .copied()
                        .map(u64::from),
                    new_price_pln: payload
                        .other_price
                        .get(&Currency::Pln)
                        .copied()
                        .map(u64::from),
                    new_price_try: payload
                        .other_price
                        .get(&Currency::Try)
                        .copied()
                        .map(u64::from),
                    new_price_jpy: payload
                        .other_price
                        .get(&Currency::Jpy)
                        .copied()
                        .map(u64::from),
                    new_price_czk: payload
                        .other_price
                        .get(&Currency::Czk)
                        .copied()
                        .map(u64::from),
                    new_price_rub: payload
                        .other_price
                        .get(&Currency::Rub)
                        .copied()
                        .map(u64::from),
                    new_price_aed: payload
                        .other_price
                        .get(&Currency::Aed)
                        .copied()
                        .map(u64::from),
                    new_price_sar: payload
                        .other_price
                        .get(&Currency::Sar)
                        .copied()
                        .map(u64::from),
                    new_price_hkd: payload
                        .other_price
                        .get(&Currency::Hkd)
                        .copied()
                        .map(u64::from),
                    new_price_sgd: payload
                        .other_price
                        .get(&Currency::Sgd)
                        .copied()
                        .map(u64::from),
                    new_price_chf: payload
                        .other_price
                        .get(&Currency::Chf)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_min_native: payload
                        .native_price_estimate_min
                        .map(PriceRecord::from),
                    new_price_estimate_min_eur: payload
                        .other_price_estimate_min
                        .get(&Currency::Eur)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_min_usd: payload
                        .other_price_estimate_min
                        .get(&Currency::Usd)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_min_gbp: payload
                        .other_price_estimate_min
                        .get(&Currency::Gbp)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_min_aud: payload
                        .other_price_estimate_min
                        .get(&Currency::Aud)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_min_cad: payload
                        .other_price_estimate_min
                        .get(&Currency::Cad)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_min_nzd: payload
                        .other_price_estimate_min
                        .get(&Currency::Nzd)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_min_cny: payload
                        .other_price_estimate_min
                        .get(&Currency::Cny)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_min_brl: payload
                        .other_price_estimate_min
                        .get(&Currency::Brl)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_min_pln: payload
                        .other_price_estimate_min
                        .get(&Currency::Pln)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_min_try: payload
                        .other_price_estimate_min
                        .get(&Currency::Try)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_min_jpy: payload
                        .other_price_estimate_min
                        .get(&Currency::Jpy)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_min_czk: payload
                        .other_price_estimate_min
                        .get(&Currency::Czk)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_min_rub: payload
                        .other_price_estimate_min
                        .get(&Currency::Rub)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_min_aed: payload
                        .other_price_estimate_min
                        .get(&Currency::Aed)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_min_sar: payload
                        .other_price_estimate_min
                        .get(&Currency::Sar)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_min_hkd: payload
                        .other_price_estimate_min
                        .get(&Currency::Hkd)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_min_sgd: payload
                        .other_price_estimate_min
                        .get(&Currency::Sgd)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_min_chf: payload
                        .other_price_estimate_min
                        .get(&Currency::Chf)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_max_native: payload
                        .native_price_estimate_max
                        .map(PriceRecord::from),
                    new_price_estimate_max_eur: payload
                        .other_price_estimate_max
                        .get(&Currency::Eur)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_max_usd: payload
                        .other_price_estimate_max
                        .get(&Currency::Usd)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_max_gbp: payload
                        .other_price_estimate_max
                        .get(&Currency::Gbp)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_max_aud: payload
                        .other_price_estimate_max
                        .get(&Currency::Aud)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_max_cad: payload
                        .other_price_estimate_max
                        .get(&Currency::Cad)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_max_nzd: payload
                        .other_price_estimate_max
                        .get(&Currency::Nzd)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_max_cny: payload
                        .other_price_estimate_max
                        .get(&Currency::Cny)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_max_brl: payload
                        .other_price_estimate_max
                        .get(&Currency::Brl)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_max_pln: payload
                        .other_price_estimate_max
                        .get(&Currency::Pln)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_max_try: payload
                        .other_price_estimate_max
                        .get(&Currency::Try)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_max_jpy: payload
                        .other_price_estimate_max
                        .get(&Currency::Jpy)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_max_czk: payload
                        .other_price_estimate_max
                        .get(&Currency::Czk)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_max_rub: payload
                        .other_price_estimate_max
                        .get(&Currency::Rub)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_max_aed: payload
                        .other_price_estimate_max
                        .get(&Currency::Aed)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_max_sar: payload
                        .other_price_estimate_max
                        .get(&Currency::Sar)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_max_hkd: payload
                        .other_price_estimate_max
                        .get(&Currency::Hkd)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_max_sgd: payload
                        .other_price_estimate_max
                        .get(&Currency::Sgd)
                        .copied()
                        .map(u64::from),
                    new_price_estimate_max_chf: payload
                        .other_price_estimate_max
                        .get(&Currency::Chf)
                        .copied()
                        .map(u64::from),
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
                    new_state: Some(payload.state.into()),
                    old_state: None,
                    url: Some(payload.url),
                    view_url: Some(payload.view_url),
                    images: Some(
                        payload
                            .images
                            .into_iter()
                            .map(ProductImageRecord::from)
                            .collect(),
                    ),
                    auction_start: payload.auction_start,
                    auction_end: payload.auction_end,
                    timestamp: domain.timestamp,
                }
            }
            ProductDomainEventPayload::StateChanged(payload) => mk_state_event_record(
                payload.new_state.into(),
                payload.old_state.into(),
                pk,
                sk,
                product_id,
                event_id,
                event_type,
                shop_id,
                payload.seller_id,
                shops_product_id,
                domain.timestamp,
            ),
            ProductDomainEventPayload::PriceChanged(payload) => mk_price_change_event_record(
                payload.clone(),
                pk,
                sk,
                product_id,
                event_id,
                event_type,
                shop_id,
                payload.seller_id,
                shops_product_id,
                domain.timestamp,
            ),
            ProductDomainEventPayload::EstimatePriceChanged(payload) => {
                let mut rec = mk_empty_event_record(
                    pk,
                    sk,
                    product_id,
                    event_id,
                    event_type,
                    shop_id,
                    payload.seller_id,
                    shops_product_id,
                    domain.timestamp,
                );
                rec.new_price_estimate_min_native =
                    payload.native_price_estimate_min.map(PriceRecord::from);
                rec.new_price_estimate_min_eur = payload
                    .other_price_estimate_min
                    .get(&Currency::Eur)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_min_usd = payload
                    .other_price_estimate_min
                    .get(&Currency::Usd)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_min_gbp = payload
                    .other_price_estimate_min
                    .get(&Currency::Gbp)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_min_aud = payload
                    .other_price_estimate_min
                    .get(&Currency::Aud)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_min_cad = payload
                    .other_price_estimate_min
                    .get(&Currency::Cad)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_min_nzd = payload
                    .other_price_estimate_min
                    .get(&Currency::Nzd)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_min_cny = payload
                    .other_price_estimate_min
                    .get(&Currency::Cny)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_min_brl = payload
                    .other_price_estimate_min
                    .get(&Currency::Brl)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_min_pln = payload
                    .other_price_estimate_min
                    .get(&Currency::Pln)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_min_try = payload
                    .other_price_estimate_min
                    .get(&Currency::Try)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_min_jpy = payload
                    .other_price_estimate_min
                    .get(&Currency::Jpy)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_min_czk = payload
                    .other_price_estimate_min
                    .get(&Currency::Czk)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_min_rub = payload
                    .other_price_estimate_min
                    .get(&Currency::Rub)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_min_aed = payload
                    .other_price_estimate_min
                    .get(&Currency::Aed)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_min_sar = payload
                    .other_price_estimate_min
                    .get(&Currency::Sar)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_min_hkd = payload
                    .other_price_estimate_min
                    .get(&Currency::Hkd)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_min_sgd = payload
                    .other_price_estimate_min
                    .get(&Currency::Sgd)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_min_chf = payload
                    .other_price_estimate_min
                    .get(&Currency::Chf)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_max_native =
                    payload.native_price_estimate_max.map(PriceRecord::from);
                rec.new_price_estimate_max_eur = payload
                    .other_price_estimate_max
                    .get(&Currency::Eur)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_max_usd = payload
                    .other_price_estimate_max
                    .get(&Currency::Usd)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_max_gbp = payload
                    .other_price_estimate_max
                    .get(&Currency::Gbp)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_max_aud = payload
                    .other_price_estimate_max
                    .get(&Currency::Aud)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_max_cad = payload
                    .other_price_estimate_max
                    .get(&Currency::Cad)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_max_nzd = payload
                    .other_price_estimate_max
                    .get(&Currency::Nzd)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_max_cny = payload
                    .other_price_estimate_max
                    .get(&Currency::Cny)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_max_brl = payload
                    .other_price_estimate_max
                    .get(&Currency::Brl)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_max_pln = payload
                    .other_price_estimate_max
                    .get(&Currency::Pln)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_max_try = payload
                    .other_price_estimate_max
                    .get(&Currency::Try)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_max_jpy = payload
                    .other_price_estimate_max
                    .get(&Currency::Jpy)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_max_czk = payload
                    .other_price_estimate_max
                    .get(&Currency::Czk)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_max_rub = payload
                    .other_price_estimate_max
                    .get(&Currency::Rub)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_max_aed = payload
                    .other_price_estimate_max
                    .get(&Currency::Aed)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_max_sar = payload
                    .other_price_estimate_max
                    .get(&Currency::Sar)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_max_hkd = payload
                    .other_price_estimate_max
                    .get(&Currency::Hkd)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_max_sgd = payload
                    .other_price_estimate_max
                    .get(&Currency::Sgd)
                    .copied()
                    .map(u64::from);
                rec.new_price_estimate_max_chf = payload
                    .other_price_estimate_max
                    .get(&Currency::Chf)
                    .copied()
                    .map(u64::from);
                rec
            }
            ProductDomainEventPayload::UrlChanged(payload) => {
                let mut rec = mk_empty_event_record(
                    pk,
                    sk,
                    product_id,
                    event_id,
                    event_type,
                    shop_id,
                    payload.seller_id,
                    shops_product_id,
                    domain.timestamp,
                );
                rec.url = Some(payload.url);
                rec.view_url = Some(payload.view_url);
                rec
            }
            ProductDomainEventPayload::ImagesChanged(payload) => {
                let mut rec = mk_empty_event_record(
                    pk,
                    sk,
                    product_id,
                    event_id,
                    event_type,
                    shop_id,
                    payload.seller_id,
                    shops_product_id,
                    domain.timestamp,
                );
                rec.images = Some(
                    payload
                        .images
                        .into_iter()
                        .map(ProductImageRecord::from)
                        .collect(),
                );
                rec
            }
            ProductDomainEventPayload::AuctionTimeChanged(payload) => {
                let mut rec = mk_empty_event_record(
                    pk,
                    sk,
                    product_id,
                    event_id,
                    event_type,
                    shop_id,
                    payload.seller_id,
                    shops_product_id,
                    domain.timestamp,
                );
                rec.auction_start = payload.auction_start;
                rec.auction_end = payload.auction_end;
                rec
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn mk_state_event_record(
    new_product_state_record: ProductStateRecord,
    old_product_state_record: ProductStateRecord,
    pk: String,
    sk: String,
    product_id: ProductId,
    event_id: EventId,
    event_type: ProductDomainEventTypeRecord,
    shop_id: ShopId,
    seller_id: ShopId,
    shops_product_id: ShopsProductId,
    timestamp: OffsetDateTime,
) -> ProductDomainEventRecord {
    ProductDomainEventRecord {
        pk,
        sk,
        product_id,
        product_slug_id: None,
        shop_slug_id: None,
        seller_slug_id: None,
        event_id,
        event_type,
        event_type_schema_version: 0,
        shop_id,
        seller_id,
        shops_product_id,
        shop_name: None,
        seller_name: None,
        shop_type: None,
        structured_address_addressline: None,
        structured_address_addressline_extra: None,
        structured_address_locality: None,
        structured_address_region: None,
        structured_address_postal_code: None,
        structured_address_country: None,
        geo_address_lat: None,
        geo_address_lon: None,
        title_native: None,
        title_de: None,
        title_en: None,
        title_fr: None,
        title_es: None,
        title_it: None,
        description_native: None,
        new_price_native: None,
        new_price_eur: None,
        new_price_usd: None,
        new_price_gbp: None,
        new_price_aud: None,
        new_price_cad: None,
        new_price_nzd: None,
        new_price_cny: None,
        new_price_brl: None,
        new_price_pln: None,
        new_price_try: None,
        new_price_jpy: None,
        new_price_czk: None,
        new_price_rub: None,
        new_price_aed: None,
        new_price_sar: None,
        new_price_hkd: None,
        new_price_sgd: None,
        new_price_chf: None,
        new_price_estimate_min_native: None,
        new_price_estimate_min_eur: None,
        new_price_estimate_min_usd: None,
        new_price_estimate_min_gbp: None,
        new_price_estimate_min_aud: None,
        new_price_estimate_min_cad: None,
        new_price_estimate_min_nzd: None,
        new_price_estimate_min_cny: None,
        new_price_estimate_min_brl: None,
        new_price_estimate_min_pln: None,
        new_price_estimate_min_try: None,
        new_price_estimate_min_jpy: None,
        new_price_estimate_min_czk: None,
        new_price_estimate_min_rub: None,
        new_price_estimate_min_aed: None,
        new_price_estimate_min_sar: None,
        new_price_estimate_min_hkd: None,
        new_price_estimate_min_sgd: None,
        new_price_estimate_min_chf: None,
        new_price_estimate_max_native: None,
        new_price_estimate_max_eur: None,
        new_price_estimate_max_usd: None,
        new_price_estimate_max_gbp: None,
        new_price_estimate_max_aud: None,
        new_price_estimate_max_cad: None,
        new_price_estimate_max_nzd: None,
        new_price_estimate_max_cny: None,
        new_price_estimate_max_brl: None,
        new_price_estimate_max_pln: None,
        new_price_estimate_max_try: None,
        new_price_estimate_max_jpy: None,
        new_price_estimate_max_czk: None,
        new_price_estimate_max_rub: None,
        new_price_estimate_max_aed: None,
        new_price_estimate_max_sar: None,
        new_price_estimate_max_hkd: None,
        new_price_estimate_max_sgd: None,
        new_price_estimate_max_chf: None,
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
        new_state: Some(new_product_state_record),
        old_state: Some(old_product_state_record),
        url: None,
        view_url: None,
        images: None,
        auction_start: None,
        auction_end: None,
        timestamp,
    }
}

#[allow(clippy::too_many_arguments)]
fn mk_price_change_event_record(
    payload: ProductPriceChangeDomainEventPayload,
    pk: String,
    sk: String,
    product_id: ProductId,
    event_id: EventId,
    event_type: ProductDomainEventTypeRecord,
    shop_id: ShopId,
    seller_id: ShopId,
    shops_product_id: ShopsProductId,
    timestamp: OffsetDateTime,
) -> ProductDomainEventRecord {
    ProductDomainEventRecord {
        pk,
        sk,
        product_id,
        shop_slug_id: None,
        seller_slug_id: None,
        product_slug_id: None,
        event_id,
        event_type,
        event_type_schema_version: 0,
        shop_id,
        seller_id,
        shops_product_id,
        shop_name: None,
        seller_name: None,
        shop_type: None,
        structured_address_addressline: None,
        structured_address_addressline_extra: None,
        structured_address_locality: None,
        structured_address_region: None,
        structured_address_postal_code: None,
        structured_address_country: None,
        geo_address_lat: None,
        geo_address_lon: None,
        title_native: None,
        title_de: None,
        title_en: None,
        title_fr: None,
        title_es: None,
        title_it: None,
        description_native: None,
        new_price_native: payload.new_native_price.map(PriceRecord::from),
        new_price_eur: payload
            .new_other_price
            .get(&Currency::Eur)
            .copied()
            .map(u64::from),
        new_price_usd: payload
            .new_other_price
            .get(&Currency::Usd)
            .copied()
            .map(u64::from),
        new_price_gbp: payload
            .new_other_price
            .get(&Currency::Gbp)
            .copied()
            .map(u64::from),
        new_price_aud: payload
            .new_other_price
            .get(&Currency::Aud)
            .copied()
            .map(u64::from),
        new_price_cad: payload
            .new_other_price
            .get(&Currency::Cad)
            .copied()
            .map(u64::from),
        new_price_nzd: payload
            .new_other_price
            .get(&Currency::Nzd)
            .copied()
            .map(u64::from),
        new_price_cny: payload
            .new_other_price
            .get(&Currency::Cny)
            .copied()
            .map(u64::from),
        new_price_brl: payload
            .new_other_price
            .get(&Currency::Brl)
            .copied()
            .map(u64::from),
        new_price_pln: payload
            .new_other_price
            .get(&Currency::Pln)
            .copied()
            .map(u64::from),
        new_price_try: payload
            .new_other_price
            .get(&Currency::Try)
            .copied()
            .map(u64::from),
        new_price_jpy: payload
            .new_other_price
            .get(&Currency::Jpy)
            .copied()
            .map(u64::from),
        new_price_czk: payload
            .new_other_price
            .get(&Currency::Czk)
            .copied()
            .map(u64::from),
        new_price_rub: payload
            .new_other_price
            .get(&Currency::Rub)
            .copied()
            .map(u64::from),
        new_price_aed: payload
            .new_other_price
            .get(&Currency::Aed)
            .copied()
            .map(u64::from),
        new_price_sar: payload
            .new_other_price
            .get(&Currency::Sar)
            .copied()
            .map(u64::from),
        new_price_hkd: payload
            .new_other_price
            .get(&Currency::Hkd)
            .copied()
            .map(u64::from),
        new_price_sgd: payload
            .new_other_price
            .get(&Currency::Sgd)
            .copied()
            .map(u64::from),
        new_price_chf: payload
            .new_other_price
            .get(&Currency::Chf)
            .copied()
            .map(u64::from),
        new_price_estimate_min_native: None,
        new_price_estimate_min_eur: None,
        new_price_estimate_min_usd: None,
        new_price_estimate_min_gbp: None,
        new_price_estimate_min_aud: None,
        new_price_estimate_min_cad: None,
        new_price_estimate_min_nzd: None,
        new_price_estimate_min_cny: None,
        new_price_estimate_min_brl: None,
        new_price_estimate_min_pln: None,
        new_price_estimate_min_try: None,
        new_price_estimate_min_jpy: None,
        new_price_estimate_min_czk: None,
        new_price_estimate_min_rub: None,
        new_price_estimate_min_aed: None,
        new_price_estimate_min_sar: None,
        new_price_estimate_min_hkd: None,
        new_price_estimate_min_sgd: None,
        new_price_estimate_min_chf: None,
        new_price_estimate_max_native: None,
        new_price_estimate_max_eur: None,
        new_price_estimate_max_usd: None,
        new_price_estimate_max_gbp: None,
        new_price_estimate_max_aud: None,
        new_price_estimate_max_cad: None,
        new_price_estimate_max_nzd: None,
        new_price_estimate_max_cny: None,
        new_price_estimate_max_brl: None,
        new_price_estimate_max_pln: None,
        new_price_estimate_max_try: None,
        new_price_estimate_max_jpy: None,
        new_price_estimate_max_czk: None,
        new_price_estimate_max_rub: None,
        new_price_estimate_max_aed: None,
        new_price_estimate_max_sar: None,
        new_price_estimate_max_hkd: None,
        new_price_estimate_max_sgd: None,
        new_price_estimate_max_chf: None,
        old_price_native: payload.old_native_price.map(PriceRecord::from),
        old_price_eur: payload
            .old_other_price
            .get(&Currency::Eur)
            .copied()
            .map(u64::from),
        old_price_usd: payload
            .old_other_price
            .get(&Currency::Usd)
            .copied()
            .map(u64::from),
        old_price_gbp: payload
            .old_other_price
            .get(&Currency::Gbp)
            .copied()
            .map(u64::from),
        old_price_aud: payload
            .old_other_price
            .get(&Currency::Aud)
            .copied()
            .map(u64::from),
        old_price_cad: payload
            .old_other_price
            .get(&Currency::Cad)
            .copied()
            .map(u64::from),
        old_price_nzd: payload
            .old_other_price
            .get(&Currency::Nzd)
            .copied()
            .map(u64::from),
        old_price_cny: payload
            .old_other_price
            .get(&Currency::Cny)
            .copied()
            .map(u64::from),
        old_price_brl: payload
            .old_other_price
            .get(&Currency::Brl)
            .copied()
            .map(u64::from),
        old_price_pln: payload
            .old_other_price
            .get(&Currency::Pln)
            .copied()
            .map(u64::from),
        old_price_try: payload
            .old_other_price
            .get(&Currency::Try)
            .copied()
            .map(u64::from),
        old_price_jpy: payload
            .old_other_price
            .get(&Currency::Jpy)
            .copied()
            .map(u64::from),
        old_price_czk: payload
            .old_other_price
            .get(&Currency::Czk)
            .copied()
            .map(u64::from),
        old_price_rub: payload
            .old_other_price
            .get(&Currency::Rub)
            .copied()
            .map(u64::from),
        old_price_aed: payload
            .old_other_price
            .get(&Currency::Aed)
            .copied()
            .map(u64::from),
        old_price_sar: payload
            .old_other_price
            .get(&Currency::Sar)
            .copied()
            .map(u64::from),
        old_price_hkd: payload
            .old_other_price
            .get(&Currency::Hkd)
            .copied()
            .map(u64::from),
        old_price_sgd: payload
            .old_other_price
            .get(&Currency::Sgd)
            .copied()
            .map(u64::from),
        old_price_chf: payload
            .old_other_price
            .get(&Currency::Chf)
            .copied()
            .map(u64::from),
        new_state: None,
        old_state: None,
        url: None,
        view_url: None,
        images: None,
        auction_start: None,
        auction_end: None,
        timestamp,
    }
}

#[allow(clippy::too_many_arguments)]
fn mk_empty_event_record(
    pk: String,
    sk: String,
    product_id: ProductId,
    event_id: EventId,
    event_type: ProductDomainEventTypeRecord,
    shop_id: ShopId,
    seller_id: ShopId,
    shops_product_id: ShopsProductId,
    timestamp: OffsetDateTime,
) -> ProductDomainEventRecord {
    ProductDomainEventRecord {
        pk,
        sk,
        product_id,
        product_slug_id: None,
        shop_slug_id: None,
        seller_slug_id: None,
        event_id,
        event_type,
        event_type_schema_version: 0,
        shop_id,
        seller_id,
        shops_product_id,
        shop_name: None,
        seller_name: None,
        shop_type: None,
        structured_address_addressline: None,
        structured_address_addressline_extra: None,
        structured_address_locality: None,
        structured_address_region: None,
        structured_address_postal_code: None,
        structured_address_country: None,
        geo_address_lat: None,
        geo_address_lon: None,
        title_native: None,
        title_de: None,
        title_en: None,
        title_fr: None,
        title_es: None,
        title_it: None,
        description_native: None,
        new_price_native: None,
        new_price_eur: None,
        new_price_usd: None,
        new_price_gbp: None,
        new_price_aud: None,
        new_price_cad: None,
        new_price_nzd: None,
        new_price_cny: None,
        new_price_brl: None,
        new_price_pln: None,
        new_price_try: None,
        new_price_jpy: None,
        new_price_czk: None,
        new_price_rub: None,
        new_price_aed: None,
        new_price_sar: None,
        new_price_hkd: None,
        new_price_sgd: None,
        new_price_chf: None,
        new_price_estimate_min_native: None,
        new_price_estimate_min_eur: None,
        new_price_estimate_min_usd: None,
        new_price_estimate_min_gbp: None,
        new_price_estimate_min_aud: None,
        new_price_estimate_min_cad: None,
        new_price_estimate_min_nzd: None,
        new_price_estimate_min_cny: None,
        new_price_estimate_min_brl: None,
        new_price_estimate_min_pln: None,
        new_price_estimate_min_try: None,
        new_price_estimate_min_jpy: None,
        new_price_estimate_min_czk: None,
        new_price_estimate_min_rub: None,
        new_price_estimate_min_aed: None,
        new_price_estimate_min_sar: None,
        new_price_estimate_min_hkd: None,
        new_price_estimate_min_sgd: None,
        new_price_estimate_min_chf: None,
        new_price_estimate_max_native: None,
        new_price_estimate_max_eur: None,
        new_price_estimate_max_usd: None,
        new_price_estimate_max_gbp: None,
        new_price_estimate_max_aud: None,
        new_price_estimate_max_cad: None,
        new_price_estimate_max_nzd: None,
        new_price_estimate_max_cny: None,
        new_price_estimate_max_brl: None,
        new_price_estimate_max_pln: None,
        new_price_estimate_max_try: None,
        new_price_estimate_max_jpy: None,
        new_price_estimate_max_czk: None,
        new_price_estimate_max_rub: None,
        new_price_estimate_max_aed: None,
        new_price_estimate_max_sar: None,
        new_price_estimate_max_hkd: None,
        new_price_estimate_max_sgd: None,
        new_price_estimate_max_chf: None,
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
        new_state: None,
        old_state: None,
        url: None,
        view_url: None,
        images: None,
        auction_start: None,
        auction_end: None,
        timestamp,
    }
}

impl TryFrom<ProductDomainEventRecord> for ProductDomainEvent {
    type Error = MissingPersistenceField;

    fn try_from(record: ProductDomainEventRecord) -> Result<Self, Self::Error> {
        let shop_id = record.shop_id;
        let seller_id = record.seller_id;
        let shops_product_id = record.shops_product_id;
        let mut new_other_price = HashMap::with_capacity(Currency::COUNT);
        if let Some(amount_eur) = record.new_price_eur {
            new_other_price.insert(Currency::Eur, amount_eur.into());
        }
        if let Some(amount_gbp) = record.new_price_gbp {
            new_other_price.insert(Currency::Gbp, amount_gbp.into());
        }
        if let Some(amount_usd) = record.new_price_usd {
            new_other_price.insert(Currency::Usd, amount_usd.into());
        }
        if let Some(amount_aud) = record.new_price_aud {
            new_other_price.insert(Currency::Aud, amount_aud.into());
        }
        if let Some(amount_cad) = record.new_price_cad {
            new_other_price.insert(Currency::Cad, amount_cad.into());
        }
        if let Some(amount_nzd) = record.new_price_nzd {
            new_other_price.insert(Currency::Nzd, amount_nzd.into());
        }
        if let Some(val) = record.new_price_cny {
            new_other_price.insert(Currency::Cny, val.into());
        }
        if let Some(val) = record.new_price_brl {
            new_other_price.insert(Currency::Brl, val.into());
        }
        if let Some(val) = record.new_price_pln {
            new_other_price.insert(Currency::Pln, val.into());
        }
        if let Some(val) = record.new_price_try {
            new_other_price.insert(Currency::Try, val.into());
        }
        if let Some(val) = record.new_price_jpy {
            new_other_price.insert(Currency::Jpy, val.into());
        }
        if let Some(val) = record.new_price_czk {
            new_other_price.insert(Currency::Czk, val.into());
        }
        if let Some(val) = record.new_price_rub {
            new_other_price.insert(Currency::Rub, val.into());
        }
        if let Some(val) = record.new_price_aed {
            new_other_price.insert(Currency::Aed, val.into());
        }
        if let Some(val) = record.new_price_sar {
            new_other_price.insert(Currency::Sar, val.into());
        }
        if let Some(val) = record.new_price_hkd {
            new_other_price.insert(Currency::Hkd, val.into());
        }
        if let Some(val) = record.new_price_sgd {
            new_other_price.insert(Currency::Sgd, val.into());
        }
        if let Some(val) = record.new_price_chf {
            new_other_price.insert(Currency::Chf, val.into());
        }

        let mut old_other_price = HashMap::with_capacity(Currency::COUNT);
        if let Some(amount_eur) = record.old_price_eur {
            old_other_price.insert(Currency::Eur, amount_eur.into());
        }
        if let Some(amount_gbp) = record.old_price_gbp {
            old_other_price.insert(Currency::Gbp, amount_gbp.into());
        }
        if let Some(amount_usd) = record.old_price_usd {
            old_other_price.insert(Currency::Usd, amount_usd.into());
        }
        if let Some(amount_aud) = record.old_price_aud {
            old_other_price.insert(Currency::Aud, amount_aud.into());
        }
        if let Some(amount_cad) = record.old_price_cad {
            old_other_price.insert(Currency::Cad, amount_cad.into());
        }
        if let Some(amount_nzd) = record.old_price_nzd {
            old_other_price.insert(Currency::Nzd, amount_nzd.into());
        }
        if let Some(val) = record.old_price_cny {
            old_other_price.insert(Currency::Cny, val.into());
        }
        if let Some(val) = record.old_price_brl {
            old_other_price.insert(Currency::Brl, val.into());
        }
        if let Some(val) = record.old_price_pln {
            old_other_price.insert(Currency::Pln, val.into());
        }
        if let Some(val) = record.old_price_try {
            old_other_price.insert(Currency::Try, val.into());
        }
        if let Some(val) = record.old_price_jpy {
            old_other_price.insert(Currency::Jpy, val.into());
        }
        if let Some(val) = record.old_price_czk {
            old_other_price.insert(Currency::Czk, val.into());
        }
        if let Some(val) = record.old_price_rub {
            old_other_price.insert(Currency::Rub, val.into());
        }
        if let Some(val) = record.old_price_aed {
            old_other_price.insert(Currency::Aed, val.into());
        }
        if let Some(val) = record.old_price_sar {
            old_other_price.insert(Currency::Sar, val.into());
        }
        if let Some(val) = record.old_price_hkd {
            old_other_price.insert(Currency::Hkd, val.into());
        }
        if let Some(val) = record.old_price_sgd {
            old_other_price.insert(Currency::Sgd, val.into());
        }
        if let Some(val) = record.old_price_chf {
            old_other_price.insert(Currency::Chf, val.into());
        }

        let mut new_other_price_estimate_min = HashMap::with_capacity(Currency::COUNT);
        if let Some(amount_eur) = record.new_price_estimate_min_eur {
            new_other_price_estimate_min.insert(Currency::Eur, amount_eur.into());
        }
        if let Some(amount_gbp) = record.new_price_estimate_min_gbp {
            new_other_price_estimate_min.insert(Currency::Gbp, amount_gbp.into());
        }
        if let Some(amount_usd) = record.new_price_estimate_min_usd {
            new_other_price_estimate_min.insert(Currency::Usd, amount_usd.into());
        }
        if let Some(amount_aud) = record.new_price_estimate_min_aud {
            new_other_price_estimate_min.insert(Currency::Aud, amount_aud.into());
        }
        if let Some(amount_cad) = record.new_price_estimate_min_cad {
            new_other_price_estimate_min.insert(Currency::Cad, amount_cad.into());
        }
        if let Some(amount_nzd) = record.new_price_estimate_min_nzd {
            new_other_price_estimate_min.insert(Currency::Nzd, amount_nzd.into());
        }
        if let Some(val) = record.new_price_estimate_min_cny {
            new_other_price_estimate_min.insert(Currency::Cny, val.into());
        }
        if let Some(val) = record.new_price_estimate_min_brl {
            new_other_price_estimate_min.insert(Currency::Brl, val.into());
        }
        if let Some(val) = record.new_price_estimate_min_pln {
            new_other_price_estimate_min.insert(Currency::Pln, val.into());
        }
        if let Some(val) = record.new_price_estimate_min_try {
            new_other_price_estimate_min.insert(Currency::Try, val.into());
        }
        if let Some(val) = record.new_price_estimate_min_jpy {
            new_other_price_estimate_min.insert(Currency::Jpy, val.into());
        }
        if let Some(val) = record.new_price_estimate_min_czk {
            new_other_price_estimate_min.insert(Currency::Czk, val.into());
        }
        if let Some(val) = record.new_price_estimate_min_rub {
            new_other_price_estimate_min.insert(Currency::Rub, val.into());
        }
        if let Some(val) = record.new_price_estimate_min_aed {
            new_other_price_estimate_min.insert(Currency::Aed, val.into());
        }
        if let Some(val) = record.new_price_estimate_min_sar {
            new_other_price_estimate_min.insert(Currency::Sar, val.into());
        }
        if let Some(val) = record.new_price_estimate_min_hkd {
            new_other_price_estimate_min.insert(Currency::Hkd, val.into());
        }
        if let Some(val) = record.new_price_estimate_min_sgd {
            new_other_price_estimate_min.insert(Currency::Sgd, val.into());
        }
        if let Some(val) = record.new_price_estimate_min_chf {
            new_other_price_estimate_min.insert(Currency::Chf, val.into());
        }

        let mut new_other_price_estimate_max = HashMap::with_capacity(Currency::COUNT);
        if let Some(amount_eur) = record.new_price_estimate_max_eur {
            new_other_price_estimate_max.insert(Currency::Eur, amount_eur.into());
        }
        if let Some(amount_gbp) = record.new_price_estimate_max_gbp {
            new_other_price_estimate_max.insert(Currency::Gbp, amount_gbp.into());
        }
        if let Some(amount_usd) = record.new_price_estimate_max_usd {
            new_other_price_estimate_max.insert(Currency::Usd, amount_usd.into());
        }
        if let Some(amount_aud) = record.new_price_estimate_max_aud {
            new_other_price_estimate_max.insert(Currency::Aud, amount_aud.into());
        }
        if let Some(amount_cad) = record.new_price_estimate_max_cad {
            new_other_price_estimate_max.insert(Currency::Cad, amount_cad.into());
        }
        if let Some(amount_nzd) = record.new_price_estimate_max_nzd {
            new_other_price_estimate_max.insert(Currency::Nzd, amount_nzd.into());
        }
        if let Some(val) = record.new_price_estimate_max_cny {
            new_other_price_estimate_max.insert(Currency::Cny, val.into());
        }
        if let Some(val) = record.new_price_estimate_max_brl {
            new_other_price_estimate_max.insert(Currency::Brl, val.into());
        }
        if let Some(val) = record.new_price_estimate_max_pln {
            new_other_price_estimate_max.insert(Currency::Pln, val.into());
        }
        if let Some(val) = record.new_price_estimate_max_try {
            new_other_price_estimate_max.insert(Currency::Try, val.into());
        }
        if let Some(val) = record.new_price_estimate_max_jpy {
            new_other_price_estimate_max.insert(Currency::Jpy, val.into());
        }
        if let Some(val) = record.new_price_estimate_max_czk {
            new_other_price_estimate_max.insert(Currency::Czk, val.into());
        }
        if let Some(val) = record.new_price_estimate_max_rub {
            new_other_price_estimate_max.insert(Currency::Rub, val.into());
        }
        if let Some(val) = record.new_price_estimate_max_aed {
            new_other_price_estimate_max.insert(Currency::Aed, val.into());
        }
        if let Some(val) = record.new_price_estimate_max_sar {
            new_other_price_estimate_max.insert(Currency::Sar, val.into());
        }
        if let Some(val) = record.new_price_estimate_max_hkd {
            new_other_price_estimate_max.insert(Currency::Hkd, val.into());
        }
        if let Some(val) = record.new_price_estimate_max_sgd {
            new_other_price_estimate_max.insert(Currency::Sgd, val.into());
        }
        if let Some(val) = record.new_price_estimate_max_chf {
            new_other_price_estimate_max.insert(Currency::Chf, val.into());
        }

        let event = Event {
            aggregate_id: record.product_id,
            event_id: record.event_id,
            timestamp: record.timestamp,
            payload: match record.event_type {
                ProductDomainEventTypeRecord::DomainCreated => {
                    ProductDomainEventPayload::Created(ProductCreatedDomainEventPayload {
                        product_slug_id: record.product_slug_id.ok_or(
                            MissingPersistenceField::new(
                                field!(product_slug_id@ProductDomainEventRecord),
                            ),
                        )?,
                        shop_slug_id: record.shop_slug_id.ok_or(MissingPersistenceField::new(
                            field!(shop_slug_id@ProductDomainEventRecord),
                        ))?,
                        seller_slug_id: record.seller_slug_id.ok_or(
                            MissingPersistenceField::new(
                                field!(seller_slug_id@ProductDomainEventRecord),
                            ),
                        )?,
                        shop_id,
                        seller_id,
                        shops_product_id,
                        shop_name: record.shop_name.map(ShopName::from).ok_or(
                            MissingPersistenceField::new(
                                field!(shop_name@ProductDomainEventRecord),
                            ),
                        )?,
                        seller_name: record.seller_name.map(ShopName::from).ok_or(
                            MissingPersistenceField::new(
                                field!(seller_name@ProductDomainEventRecord),
                            ),
                        )?,
                        shop_type: record.shop_type.map(Into::into).ok_or(
                            MissingPersistenceField::new(
                                field!(shop_type@ProductDomainEventRecord),
                            ),
                        )?,
                        structured_address: structured_address_from_record(
                            record.structured_address_addressline,
                            record.structured_address_addressline_extra,
                            record.structured_address_locality,
                            record.structured_address_region,
                            record.structured_address_postal_code,
                            record.structured_address_country,
                        ),
                        geo_address: geo_address_from_record(
                            record.geo_address_lat,
                            record.geo_address_lon,
                        ),
                        native_title: record.title_native.map(Localized::from).ok_or(
                            MissingPersistenceField::new(
                                field!(title_native@ProductDomainEventRecord),
                            ),
                        )?,
                        native_description: record.description_native.map(Localized::from),
                        native_price: record.new_price_native.map(Price::from),
                        other_price: new_other_price,
                        native_price_estimate_min: record
                            .new_price_estimate_min_native
                            .map(Price::from),
                        other_price_estimate_min: new_other_price_estimate_min,
                        native_price_estimate_max: record
                            .new_price_estimate_max_native
                            .map(Price::from),
                        other_price_estimate_max: new_other_price_estimate_max,
                        state: record.new_state.map(ProductState::from).ok_or(
                            MissingPersistenceField::new(
                                field!(new_state@ProductDomainEventRecord),
                            ),
                        )?,
                        url: record.url.clone().ok_or(MissingPersistenceField::new(
                            field!(url@ProductDomainEventRecord),
                        ))?,
                        view_url: {
                            let raw = record.url.ok_or(MissingPersistenceField::new(
                                field!(url@ProductDomainEventRecord),
                            ))?;
                            record.view_url.unwrap_or_else(|| append_utm_params(raw))
                        },
                        images: record
                            .images
                            .unwrap_or_default()
                            .into_iter()
                            .map(ProductImage::from)
                            .collect(),
                        auction_start: record.auction_start,
                        auction_end: record.auction_end,
                    })
                }
                ProductDomainEventTypeRecord::DomainStateChanged => {
                    ProductDomainEventPayload::StateChanged(ProductStateChangeDomainEventPayload {
                        shop_id,
                        seller_id,
                        shops_product_id,
                        old_state: record.old_state.map(ProductState::from).ok_or(
                            MissingPersistenceField::new(
                                field!(old_state@ProductDomainEventRecord),
                            ),
                        )?,
                        new_state: record.new_state.map(ProductState::from).ok_or(
                            MissingPersistenceField::new(
                                field!(new_state@ProductDomainEventRecord),
                            ),
                        )?,
                    })
                }
                ProductDomainEventTypeRecord::DomainPriceChanged => {
                    ProductDomainEventPayload::PriceChanged(ProductPriceChangeDomainEventPayload {
                        shop_id,
                        seller_id,
                        shops_product_id,
                        new_native_price: record.new_price_native.map(Price::from),
                        new_other_price,
                        old_native_price: record.old_price_native.map(Price::from),
                        old_other_price,
                    })
                }
                ProductDomainEventTypeRecord::DomainEstimatePriceChanged => {
                    ProductDomainEventPayload::EstimatePriceChanged(
                        ProductEstimatePriceChangeDomainEventPayload {
                            shop_id,
                            seller_id,
                            shops_product_id,
                            native_price_estimate_min: record
                                .new_price_estimate_min_native
                                .map(Price::from),
                            other_price_estimate_min: new_other_price_estimate_min,
                            native_price_estimate_max: record
                                .new_price_estimate_max_native
                                .map(Price::from),
                            other_price_estimate_max: new_other_price_estimate_max,
                        },
                    )
                }
                ProductDomainEventTypeRecord::DomainUrlChanged => {
                    let raw_url = record.url.ok_or(MissingPersistenceField::new(
                        field!(url@ProductDomainEventRecord),
                    ))?;
                    let view_url = record
                        .view_url
                        .unwrap_or_else(|| append_utm_params(raw_url.clone()));
                    ProductDomainEventPayload::UrlChanged(ProductUrlChangeDomainEventPayload {
                        shop_id,
                        seller_id,
                        shops_product_id,
                        url: raw_url,
                        view_url,
                    })
                }
                ProductDomainEventTypeRecord::DomainImagesChanged => {
                    ProductDomainEventPayload::ImagesChanged(
                        ProductImagesChangeDomainEventPayload {
                            shop_id,
                            seller_id,
                            shops_product_id,
                            images: record
                                .images
                                .unwrap_or_default()
                                .into_iter()
                                .map(ProductImage::from)
                                .collect(),
                        },
                    )
                }
                ProductDomainEventTypeRecord::DomainAuctionTimeChanged => {
                    ProductDomainEventPayload::AuctionTimeChanged(
                        ProductAuctionTimeChangeDomainEventPayload {
                            shop_id,
                            seller_id,
                            shops_product_id,
                            auction_start: record.auction_start,
                            auction_end: record.auction_end,
                        },
                    )
                }
            },
        };
        Ok(event)
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ProductDomainEventRecord {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config.fake_with_rng::<ProductDomainEvent, _>(rng).into()
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::dynamodb::product_event_record::domain::ProductDomainEventRecord;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_get_product_event_record() {
            let _ = Faker.fake::<ProductDomainEventRecord>();
        }
    }
}

#[cfg(all(test, feature = "test-data"))]
mod tests {
    use super::*;
    use crate::core::product_event::ProductDomainEvent;
    use crate::core::product_event::domain::{
        ProductCreatedDomainEventPayload, ProductDomainEventPayload,
        ProductUrlChangeDomainEventPayload,
    };
    use fake::{Fake, Faker};
    use time::OffsetDateTime;
    use url::Url;

    #[test]
    fn should_append_utm_params_to_view_url_when_mapping_created_event_record_to_domain_event() {
        let mut payload: ProductCreatedDomainEventPayload = Faker.fake();
        payload.url = Url::parse("https://example-shop.com/product/1").unwrap();
        let event = ProductDomainEvent {
            aggregate_id: Faker.fake(),
            event_id: Faker.fake(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::Created(payload),
        };
        // Clear view_url to test the UTM fallback path
        let mut record: ProductDomainEventRecord = event.into();
        record.view_url = None;
        let result = ProductDomainEvent::try_from(record).unwrap();

        if let ProductDomainEventPayload::Created(payload) = result.payload {
            // url must remain clean (no UTM params)
            assert!(
                payload.url.query().is_none()
                    || !payload.url.query_pairs().any(|(k, _)| k == "utm_source"),
                "url should not contain utm_source"
            );
            // view_url must contain UTM params (fallback path)
            let query: Vec<(_, _)> = payload.view_url.query_pairs().collect();
            assert!(
                query
                    .iter()
                    .any(|(k, v)| k == "utm_source" && v == "aura_historia"),
                "utm_source=aura_historia not found in view_url query params"
            );
            assert!(
                query
                    .iter()
                    .any(|(k, v)| k == "utm_medium" && v == "referral"),
                "utm_medium=referral not found in view_url query params"
            );
        } else {
            panic!("Expected ProductDomainEventPayload::Created");
        }
    }

    #[test]
    fn should_append_utm_params_to_view_url_when_mapping_url_changed_event_record_to_domain_event()
    {
        let mut payload: ProductUrlChangeDomainEventPayload = Faker.fake();
        payload.url = Url::parse("https://example-shop.com/product/2").unwrap();
        let event = ProductDomainEvent {
            aggregate_id: Faker.fake(),
            event_id: Faker.fake(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::UrlChanged(payload),
        };
        // Clear view_url to test the UTM fallback path
        let mut record: ProductDomainEventRecord = event.into();
        record.view_url = None;
        let result = ProductDomainEvent::try_from(record).unwrap();

        if let ProductDomainEventPayload::UrlChanged(payload) = result.payload {
            // url must remain clean (no UTM params)
            assert!(
                payload.url.query().is_none()
                    || !payload.url.query_pairs().any(|(k, _)| k == "utm_source"),
                "url should not contain utm_source"
            );
            // view_url must contain UTM params (fallback path)
            let query: Vec<(_, _)> = payload.view_url.query_pairs().collect();
            assert!(
                query
                    .iter()
                    .any(|(k, v)| k == "utm_source" && v == "aura_historia"),
                "utm_source=aura_historia not found in view_url query params"
            );
            assert!(
                query
                    .iter()
                    .any(|(k, v)| k == "utm_medium" && v == "referral"),
                "utm_medium=referral not found in view_url query params"
            );
        } else {
            panic!("Expected ProductDomainEventPayload::UrlChanged");
        }
    }
}
