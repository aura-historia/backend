use crate::core::product_event::ProductDomainEvent;
use crate::core::product_event::domain::{
    ProductCommonEventPayload, ProductCreatedDomainEventPayload, ProductDomainEventPayload,
    ProductPriceChangeDomainEventPayload, ProductPriceDiscoveryDomainEventPayload,
    ProductPriceRemovedDomainEventPayload, ProductStateChangeDomainEventPayload,
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
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use shop::dynamodb::shop_type_record::ShopTypeRecord;
use std::collections::HashMap;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct ProductDomainEventRecord {
    pub pk: String,
    pub sk: String,
    pub product_id: ProductId,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub product_slug_id: Option<SlugId<6>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_slug_id: Option<SlugId<0>>,
    pub event_id: EventId,
    pub event_type: ProductDomainEventTypeRecord,
    pub event_type_schema_version: u8,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_type: Option<ShopTypeRecord>,

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
    pub description_de: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_fr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_es: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_it: Option<String>,

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
    pub new_state: Option<ProductStateRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_state: Option<ProductStateRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<Url>,

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

#[allow(clippy::infallible_try_from)]
impl TryFrom<ProductDomainEvent> for ProductDomainEventRecord {
    type Error = std::convert::Infallible;
    fn try_from(domain: ProductDomainEvent) -> Result<Self, Self::Error> {
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
                    };

                let (
                    description_de,
                    description_en,
                    description_fr,
                    description_es,
                    description_it,
                ) = match payload.native_description {
                    Some(ref native_description) => match native_description.localization {
                        Language::De => (
                            Some(native_description.payload.to_string()),
                            None,
                            None,
                            None,
                            None,
                        ),
                        Language::En => (
                            None,
                            Some(native_description.payload.to_string()),
                            None,
                            None,
                            None,
                        ),
                        Language::Fr => (
                            None,
                            None,
                            Some(native_description.payload.to_string()),
                            None,
                            None,
                        ),
                        Language::Es => (
                            None,
                            None,
                            None,
                            Some(native_description.payload.to_string()),
                            None,
                        ),
                        Language::It => (
                            None,
                            None,
                            None,
                            None,
                            Some(native_description.payload.to_string()),
                        ),
                    },
                    None => (None, None, None, None, None),
                };

                let record = ProductDomainEventRecord {
                    pk,
                    sk,
                    product_id,
                    product_slug_id: Some(payload.product_slug_id),
                    shop_slug_id: Some(payload.shop_slug_id),
                    event_id,
                    event_type,
                    event_type_schema_version: 0,
                    shop_id,
                    shops_product_id,
                    shop_name: Some(payload.shop_name.into()),
                    shop_type: Some(payload.shop_type.into()),
                    title_native: Some(payload.native_title.into()),
                    title_de,
                    title_en,
                    title_fr,
                    title_es,
                    title_it,
                    description_native: payload.native_description.map(TextRecord::from),
                    description_de,
                    description_en,
                    description_fr,
                    description_es,
                    description_it,
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
                    old_price_native: None,
                    old_price_eur: None,
                    old_price_usd: None,
                    old_price_gbp: None,
                    old_price_aud: None,
                    old_price_cad: None,
                    old_price_nzd: None,
                    new_state: Some(payload.state.into()),
                    old_state: None,
                    url: Some(payload.url),
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
                };
                Ok(record)
            }
            ProductDomainEventPayload::StateListed(payload) => Ok(mk_state_event_record(
                ProductStateRecord::Listed,
                payload.old_state.into(),
                pk,
                sk,
                product_id,
                event_id,
                event_type,
                shop_id,
                shops_product_id,
                domain.timestamp,
            )),
            ProductDomainEventPayload::StateReserved(payload) => Ok(mk_state_event_record(
                ProductStateRecord::Reserved,
                payload.old_state.into(),
                pk,
                sk,
                product_id,
                event_id,
                event_type,
                shop_id,
                shops_product_id,
                domain.timestamp,
            )),
            ProductDomainEventPayload::StateAvailable(payload) => Ok(mk_state_event_record(
                ProductStateRecord::Available,
                payload.old_state.into(),
                pk,
                sk,
                product_id,
                event_id,
                event_type,
                shop_id,
                shops_product_id,
                domain.timestamp,
            )),
            ProductDomainEventPayload::StateSold(payload) => Ok(mk_state_event_record(
                ProductStateRecord::Sold,
                payload.old_state.into(),
                pk,
                sk,
                product_id,
                event_id,
                event_type,
                shop_id,
                shops_product_id,
                domain.timestamp,
            )),
            ProductDomainEventPayload::StateRemoved(payload) => Ok(mk_state_event_record(
                ProductStateRecord::Removed,
                payload.old_state.into(),
                pk,
                sk,
                product_id,
                event_id,
                event_type,
                shop_id,
                shops_product_id,
                domain.timestamp,
            )),
            ProductDomainEventPayload::StateUnknown(payload) => Ok(mk_state_event_record(
                ProductStateRecord::Unknown,
                payload.old_state.into(),
                pk,
                sk,
                product_id,
                event_id,
                event_type,
                shop_id,
                shops_product_id,
                domain.timestamp,
            )),
            ProductDomainEventPayload::PriceDiscovered(payload) => Ok(ProductDomainEventRecord {
                pk,
                sk,
                product_id,
                product_slug_id: None,
                shop_slug_id: None,
                event_id,
                event_type,
                event_type_schema_version: 0,
                shop_id,
                shops_product_id,
                shop_name: None,
                shop_type: None,
                title_native: None,
                title_de: None,
                title_en: None,
                title_fr: None,
                title_es: None,
                title_it: None,
                description_native: None,
                description_de: None,
                description_en: None,
                description_fr: None,
                description_es: None,
                description_it: None,
                new_price_native: Some(payload.native_price.into()),
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
                new_price_estimate_min_native: None,
                new_price_estimate_min_eur: None,
                new_price_estimate_min_usd: None,
                new_price_estimate_min_gbp: None,
                new_price_estimate_min_aud: None,
                new_price_estimate_min_cad: None,
                new_price_estimate_min_nzd: None,
                new_price_estimate_max_native: None,
                new_price_estimate_max_eur: None,
                new_price_estimate_max_usd: None,
                new_price_estimate_max_gbp: None,
                new_price_estimate_max_aud: None,
                new_price_estimate_max_cad: None,
                new_price_estimate_max_nzd: None,
                old_price_native: None,
                old_price_eur: None,
                old_price_usd: None,
                old_price_gbp: None,
                old_price_aud: None,
                old_price_cad: None,
                old_price_nzd: None,
                new_state: None,
                old_state: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
                timestamp: domain.timestamp,
            }),
            ProductDomainEventPayload::PriceIncreased(payload) => Ok(mk_price_change_event_record(
                payload,
                pk,
                sk,
                product_id,
                event_id,
                event_type,
                shop_id,
                shops_product_id,
                domain.timestamp,
            )),
            ProductDomainEventPayload::PriceDropped(payload) => Ok(mk_price_change_event_record(
                payload,
                pk,
                sk,
                product_id,
                event_id,
                event_type,
                shop_id,
                shops_product_id,
                domain.timestamp,
            )),
            ProductDomainEventPayload::PriceRemoved(payload) => Ok(ProductDomainEventRecord {
                pk,
                sk,
                product_id,
                shop_slug_id: None,
                product_slug_id: None,
                event_id,
                event_type,
                event_type_schema_version: 0,
                shop_id,
                shops_product_id,
                shop_name: None,
                shop_type: None,
                title_native: None,
                title_de: None,
                title_en: None,
                title_fr: None,
                title_es: None,
                title_it: None,
                description_native: None,
                description_de: None,
                description_en: None,
                description_fr: None,
                description_es: None,
                description_it: None,
                new_price_native: None,
                new_price_eur: None,
                new_price_usd: None,
                new_price_gbp: None,
                new_price_aud: None,
                new_price_cad: None,
                new_price_nzd: None,
                new_price_estimate_min_native: None,
                new_price_estimate_min_eur: None,
                new_price_estimate_min_usd: None,
                new_price_estimate_min_gbp: None,
                new_price_estimate_min_aud: None,
                new_price_estimate_min_cad: None,
                new_price_estimate_min_nzd: None,
                new_price_estimate_max_native: None,
                new_price_estimate_max_eur: None,
                new_price_estimate_max_usd: None,
                new_price_estimate_max_gbp: None,
                new_price_estimate_max_aud: None,
                new_price_estimate_max_cad: None,
                new_price_estimate_max_nzd: None,
                old_price_native: Some(payload.old_native_price.into()),
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
                new_state: None,
                old_state: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
                timestamp: domain.timestamp,
            }),
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
    shops_product_id: ShopsProductId,
    timestamp: OffsetDateTime,
) -> ProductDomainEventRecord {
    ProductDomainEventRecord {
        pk,
        sk,
        product_id,
        product_slug_id: None,
        shop_slug_id: None,
        event_id,
        event_type,
        event_type_schema_version: 0,
        shop_id,
        shops_product_id,
        shop_name: None,
        shop_type: None,
        title_native: None,
        title_de: None,
        title_en: None,
        title_fr: None,
        title_es: None,
        title_it: None,
        description_native: None,
        description_de: None,
        description_en: None,
        description_fr: None,
        description_es: None,
        description_it: None,
        new_price_native: None,
        new_price_eur: None,
        new_price_usd: None,
        new_price_gbp: None,
        new_price_aud: None,
        new_price_cad: None,
        new_price_nzd: None,
        new_price_estimate_min_native: None,
        new_price_estimate_min_eur: None,
        new_price_estimate_min_usd: None,
        new_price_estimate_min_gbp: None,
        new_price_estimate_min_aud: None,
        new_price_estimate_min_cad: None,
        new_price_estimate_min_nzd: None,
        new_price_estimate_max_native: None,
        new_price_estimate_max_eur: None,
        new_price_estimate_max_usd: None,
        new_price_estimate_max_gbp: None,
        new_price_estimate_max_aud: None,
        new_price_estimate_max_cad: None,
        new_price_estimate_max_nzd: None,
        old_price_native: None,
        old_price_eur: None,
        old_price_usd: None,
        old_price_gbp: None,
        old_price_aud: None,
        old_price_cad: None,
        old_price_nzd: None,
        new_state: Some(new_product_state_record),
        old_state: Some(old_product_state_record),
        url: None,
        images: None,
        auction_start: None,
        auction_end: None,
        timestamp,
    }
}

#[allow(clippy::too_many_arguments)]
fn mk_price_change_event_record(
    product_price_change_event_payload: ProductPriceChangeDomainEventPayload,
    pk: String,
    sk: String,
    product_id: ProductId,
    event_id: EventId,
    event_type: ProductDomainEventTypeRecord,
    shop_id: ShopId,
    shops_product_id: ShopsProductId,
    timestamp: OffsetDateTime,
) -> ProductDomainEventRecord {
    ProductDomainEventRecord {
        pk,
        sk,
        product_id,
        shop_slug_id: None,
        product_slug_id: None,
        event_id,
        event_type,
        event_type_schema_version: 0,
        shop_id,
        shops_product_id,
        shop_name: None,
        shop_type: None,
        title_native: None,
        title_de: None,
        title_en: None,
        title_fr: None,
        title_es: None,
        title_it: None,
        description_native: None,
        description_de: None,
        description_en: None,
        description_fr: None,
        description_es: None,
        description_it: None,
        new_price_native: Some(product_price_change_event_payload.new_native_price.into()),
        new_price_eur: product_price_change_event_payload
            .new_other_price
            .get(&Currency::Eur)
            .copied()
            .map(u64::from),
        new_price_usd: product_price_change_event_payload
            .new_other_price
            .get(&Currency::Usd)
            .copied()
            .map(u64::from),
        new_price_gbp: product_price_change_event_payload
            .new_other_price
            .get(&Currency::Gbp)
            .copied()
            .map(u64::from),
        new_price_aud: product_price_change_event_payload
            .new_other_price
            .get(&Currency::Aud)
            .copied()
            .map(u64::from),
        new_price_cad: product_price_change_event_payload
            .new_other_price
            .get(&Currency::Cad)
            .copied()
            .map(u64::from),
        new_price_nzd: product_price_change_event_payload
            .new_other_price
            .get(&Currency::Nzd)
            .copied()
            .map(u64::from),
        new_price_estimate_min_native: None,
        new_price_estimate_min_eur: None,
        new_price_estimate_min_usd: None,
        new_price_estimate_min_gbp: None,
        new_price_estimate_min_aud: None,
        new_price_estimate_min_cad: None,
        new_price_estimate_min_nzd: None,
        new_price_estimate_max_native: None,
        new_price_estimate_max_eur: None,
        new_price_estimate_max_usd: None,
        new_price_estimate_max_gbp: None,
        new_price_estimate_max_aud: None,
        new_price_estimate_max_cad: None,
        new_price_estimate_max_nzd: None,
        old_price_native: Some(product_price_change_event_payload.old_native_price.into()),
        old_price_eur: product_price_change_event_payload
            .old_other_price
            .get(&Currency::Eur)
            .copied()
            .map(u64::from),
        old_price_usd: product_price_change_event_payload
            .old_other_price
            .get(&Currency::Usd)
            .copied()
            .map(u64::from),
        old_price_gbp: product_price_change_event_payload
            .old_other_price
            .get(&Currency::Gbp)
            .copied()
            .map(u64::from),
        old_price_aud: product_price_change_event_payload
            .old_other_price
            .get(&Currency::Aud)
            .copied()
            .map(u64::from),
        old_price_cad: product_price_change_event_payload
            .old_other_price
            .get(&Currency::Cad)
            .copied()
            .map(u64::from),
        old_price_nzd: product_price_change_event_payload
            .old_other_price
            .get(&Currency::Nzd)
            .copied()
            .map(u64::from),
        new_state: None,
        old_state: None,
        url: None,
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
        let shops_product_id = record.shops_product_id;
        let mut new_other_price = HashMap::with_capacity(6);
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

        let mut old_other_price = HashMap::with_capacity(6);
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

        let mut new_other_price_estimate_min = HashMap::with_capacity(6);
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

        let mut new_other_price_estimate_max = HashMap::with_capacity(6);
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
                        shop_id,
                        shops_product_id,
                        shop_name: record.shop_name.map(ShopName::from).ok_or(
                            MissingPersistenceField::new(
                                field!(shop_name@ProductDomainEventRecord),
                            ),
                        )?,
                        shop_type: record.shop_type.map(Into::into).ok_or(
                            MissingPersistenceField::new(
                                field!(shop_type@ProductDomainEventRecord),
                            ),
                        )?,
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
                        url: record.url.ok_or(MissingPersistenceField::new(
                            field!(url@ProductDomainEventRecord),
                        ))?,
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
                ProductDomainEventTypeRecord::DomainStateListed => {
                    ProductDomainEventPayload::StateListed(ProductStateChangeDomainEventPayload {
                        shop_id,
                        shops_product_id,
                        old_state: record.old_state.map(ProductState::from).ok_or(
                            MissingPersistenceField::new(
                                field!(old_state@ProductDomainEventRecord),
                            ),
                        )?,
                    })
                }
                ProductDomainEventTypeRecord::DomainStateAvailable => {
                    ProductDomainEventPayload::StateAvailable(
                        ProductStateChangeDomainEventPayload {
                            shop_id,
                            shops_product_id,
                            old_state: record.old_state.map(ProductState::from).ok_or(
                                MissingPersistenceField::new(
                                    field!(old_state@ProductDomainEventRecord),
                                ),
                            )?,
                        },
                    )
                }
                ProductDomainEventTypeRecord::DomainStateReserved => {
                    ProductDomainEventPayload::StateReserved(ProductStateChangeDomainEventPayload {
                        shop_id,
                        shops_product_id,
                        old_state: record.old_state.map(ProductState::from).ok_or(
                            MissingPersistenceField::new(
                                field!(old_state@ProductDomainEventRecord),
                            ),
                        )?,
                    })
                }
                ProductDomainEventTypeRecord::DomainStateSold => {
                    ProductDomainEventPayload::StateSold(ProductStateChangeDomainEventPayload {
                        shop_id,
                        shops_product_id,
                        old_state: record.old_state.map(ProductState::from).ok_or(
                            MissingPersistenceField::new(
                                field!(old_state@ProductDomainEventRecord),
                            ),
                        )?,
                    })
                }
                ProductDomainEventTypeRecord::DomainStateRemoved => {
                    ProductDomainEventPayload::StateRemoved(ProductStateChangeDomainEventPayload {
                        shop_id,
                        shops_product_id,
                        old_state: record.old_state.map(ProductState::from).ok_or(
                            MissingPersistenceField::new(
                                field!(old_state@ProductDomainEventRecord),
                            ),
                        )?,
                    })
                }
                ProductDomainEventTypeRecord::DomainStateUnknown => {
                    ProductDomainEventPayload::StateUnknown(ProductStateChangeDomainEventPayload {
                        shop_id,
                        shops_product_id,
                        old_state: record.old_state.map(ProductState::from).ok_or(
                            MissingPersistenceField::new(
                                field!(old_state@ProductDomainEventRecord),
                            ),
                        )?,
                    })
                }
                ProductDomainEventTypeRecord::DomainPriceDiscovered => {
                    ProductDomainEventPayload::PriceDiscovered(
                        ProductPriceDiscoveryDomainEventPayload {
                            shop_id,
                            shops_product_id,
                            native_price: record.new_price_native.map(Price::from).ok_or(
                                MissingPersistenceField::new(
                                    field!(new_price_native@ProductDomainEventRecord),
                                ),
                            )?,
                            other_price: new_other_price,
                        },
                    )
                }
                ProductDomainEventTypeRecord::DomainPriceDropped => {
                    ProductDomainEventPayload::PriceDropped(ProductPriceChangeDomainEventPayload {
                        shop_id,
                        shops_product_id,
                        new_native_price: record.new_price_native.map(Price::from).ok_or(
                            MissingPersistenceField::new(
                                field!(new_price_native@ProductDomainEventRecord),
                            ),
                        )?,
                        new_other_price,
                        old_native_price: record.old_price_native.map(Price::from).ok_or(
                            MissingPersistenceField::new(
                                field!(old_price_native@ProductDomainEventRecord),
                            ),
                        )?,
                        old_other_price,
                    })
                }
                ProductDomainEventTypeRecord::DomainPriceIncreased => {
                    ProductDomainEventPayload::PriceIncreased(
                        ProductPriceChangeDomainEventPayload {
                            shop_id,
                            shops_product_id,
                            new_native_price: record.new_price_native.map(Price::from).ok_or(
                                MissingPersistenceField::new(
                                    field!(new_price_native@ProductDomainEventRecord),
                                ),
                            )?,
                            new_other_price,
                            old_native_price: record.old_price_native.map(Price::from).ok_or(
                                MissingPersistenceField::new(
                                    field!(old_price_native@ProductDomainEventRecord),
                                ),
                            )?,
                            old_other_price,
                        },
                    )
                }
                ProductDomainEventTypeRecord::DomainPriceRemoved => {
                    ProductDomainEventPayload::PriceRemoved(ProductPriceRemovedDomainEventPayload {
                        shop_id,
                        shops_product_id,
                        old_native_price: record.old_price_native.map(Price::from).ok_or(
                            MissingPersistenceField::new(
                                field!(old_price_native@ProductDomainEventRecord),
                            ),
                        )?,
                        old_other_price,
                    })
                }
            },
        };
        Ok(event)
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for ProductDomainEventRecord {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config
                .fake_with_rng::<ProductDomainEvent, _>(rng)
                .try_into()
                .unwrap()
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
