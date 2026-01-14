use crate::core::product_event::{
    ProductCommonEventPayload, ProductCreatedEventPayload, ProductEvent, ProductEventPayload,
    ProductPriceChangeEventPayload, ProductPriceDiscoveryEventPayload,
    ProductPriceRemovedEventPayload, ProductStateChangeEventPayload,
};
use crate::core::product_image::ProductImage;
use crate::dynamodb::product_event_type_record::ProductEventTypeRecord;
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
use field::field;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use shop::dynamodb::shop_type_record::ShopTypeRecord;
use std::collections::HashMap;
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, error};
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct ProductEventRecord {
    pub pk: String,
    pub sk: String,
    pub product_id: ProductId,
    pub event_id: EventId,
    pub event_type: ProductEventTypeRecord,
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

pub fn mk_sk(timestamp: &OffsetDateTime) -> Result<String, error::Format> {
    Ok(format!("product#event#{}", timestamp.format(&Rfc3339)?))
}

impl ProductEventRecord {
    pub fn into_product_key(self) -> ProductKey {
        ProductKey::new(self.shop_id, self.shops_product_id)
    }
}

impl HasKey for ProductEventRecord {
    type Key = ProductKey;

    fn key(&self) -> ProductKey {
        ProductKey {
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id.clone(),
        }
    }
}

impl TryFrom<ProductEvent> for ProductEventRecord {
    type Error = error::Format;
    fn try_from(domain: ProductEvent) -> Result<Self, Self::Error> {
        let shop_id = *domain.payload.shop_id();
        let shops_product_id = domain.payload.shops_product_id();
        let pk = mk_pk(&shop_id, shops_product_id);
        let sk = mk_sk(&domain.timestamp)?;
        let product_id = domain.aggregate_id;
        let event_id = domain.event_id;
        let event_type: ProductEventTypeRecord = (&domain.payload).into();
        let shops_product_id = shops_product_id.clone();

        match domain.payload {
            ProductEventPayload::Created(payload) => {
                let (title_de, title_en, title_fr, title_es) =
                    match payload.native_title.localization {
                        Language::De => (
                            Some(payload.native_title.payload.to_string()),
                            None,
                            None,
                            None,
                        ),
                        Language::En => (
                            None,
                            Some(payload.native_title.payload.to_string()),
                            None,
                            None,
                        ),
                        Language::Fr => (
                            None,
                            None,
                            Some(payload.native_title.payload.to_string()),
                            None,
                        ),
                        Language::Es => (
                            None,
                            None,
                            None,
                            Some(payload.native_title.payload.to_string()),
                        ),
                    };

                let (description_de, description_en, description_fr, description_es) =
                    match payload.native_description {
                        Some(ref native_description) => match native_description.localization {
                            Language::De => (
                                Some(native_description.payload.to_string()),
                                None,
                                None,
                                None,
                            ),
                            Language::En => (
                                None,
                                Some(native_description.payload.to_string()),
                                None,
                                None,
                            ),
                            Language::Fr => (
                                None,
                                None,
                                Some(native_description.payload.to_string()),
                                None,
                            ),
                            Language::Es => (
                                None,
                                None,
                                None,
                                Some(native_description.payload.to_string()),
                            ),
                        },
                        None => (None, None, None, None),
                    };

                let record = ProductEventRecord {
                    pk,
                    sk,
                    product_id,
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
                    description_native: payload.native_description.map(TextRecord::from),
                    description_de,
                    description_en,
                    description_fr,
                    description_es,
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
            ProductEventPayload::StateListed(payload) => Ok(mk_state_event_record(
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
            ProductEventPayload::StateReserved(payload) => Ok(mk_state_event_record(
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
            ProductEventPayload::StateAvailable(payload) => Ok(mk_state_event_record(
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
            ProductEventPayload::StateSold(payload) => Ok(mk_state_event_record(
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
            ProductEventPayload::StateRemoved(payload) => Ok(mk_state_event_record(
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
            ProductEventPayload::StateUnknown(payload) => Ok(mk_state_event_record(
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
            ProductEventPayload::PriceDiscovered(payload) => Ok(ProductEventRecord {
                pk,
                sk,
                product_id,
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
                description_native: None,
                description_de: None,
                description_en: None,
                description_fr: None,
                description_es: None,
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
            ProductEventPayload::PriceIncreased(payload) => Ok(mk_price_change_event_record(
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
            ProductEventPayload::PriceDropped(payload) => Ok(mk_price_change_event_record(
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
            ProductEventPayload::PriceRemoved(payload) => Ok(ProductEventRecord {
                pk,
                sk,
                product_id,
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
                description_native: None,
                description_de: None,
                description_en: None,
                description_fr: None,
                description_es: None,
                new_price_native: None,
                new_price_eur: None,
                new_price_usd: None,
                new_price_gbp: None,
                new_price_aud: None,
                new_price_cad: None,
                new_price_nzd: None,
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
    event_type: ProductEventTypeRecord,
    shop_id: ShopId,
    shops_product_id: ShopsProductId,
    timestamp: OffsetDateTime,
) -> ProductEventRecord {
    ProductEventRecord {
        pk,
        sk,
        product_id,
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
        description_native: None,
        description_de: None,
        description_en: None,
        description_fr: None,
        description_es: None,
        new_price_native: None,
        new_price_eur: None,
        new_price_usd: None,
        new_price_gbp: None,
        new_price_aud: None,
        new_price_cad: None,
        new_price_nzd: None,
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
    product_price_change_event_payload: ProductPriceChangeEventPayload,
    pk: String,
    sk: String,
    product_id: ProductId,
    event_id: EventId,
    event_type: ProductEventTypeRecord,
    shop_id: ShopId,
    shops_product_id: ShopsProductId,
    timestamp: OffsetDateTime,
) -> ProductEventRecord {
    ProductEventRecord {
        pk,
        sk,
        product_id,
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
        description_native: None,
        description_de: None,
        description_en: None,
        description_fr: None,
        description_es: None,
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

impl TryFrom<ProductEventRecord> for ProductEvent {
    type Error = MissingPersistenceField;

    fn try_from(record: ProductEventRecord) -> Result<Self, Self::Error> {
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

        let event = Event {
            aggregate_id: record.product_id,
            event_id: record.event_id,
            timestamp: record.timestamp,
            payload: match record.event_type {
                ProductEventTypeRecord::Created => {
                    ProductEventPayload::Created(ProductCreatedEventPayload {
                        shop_id,
                        shops_product_id,
                        shop_name: record.shop_name.map(ShopName::from).ok_or(
                            MissingPersistenceField::new(field!(shop_name@ProductEventRecord)),
                        )?,
                        shop_type: record.shop_type.map(Into::into).ok_or(
                            MissingPersistenceField::new(field!(shop_type@ProductEventRecord)),
                        )?,
                        native_title: record.title_native.map(Localized::from).ok_or(
                            MissingPersistenceField::new(field!(title_native@ProductEventRecord)),
                        )?,
                        native_description: record.description_native.map(Localized::from),
                        native_price: record.new_price_native.map(Price::from),
                        other_price: new_other_price,
                        state: record.new_state.map(ProductState::from).ok_or(
                            MissingPersistenceField::new(field!(new_state@ProductEventRecord)),
                        )?,
                        url: record
                            .url
                            .ok_or(MissingPersistenceField::new(field!(url@ProductEventRecord)))?,
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
                ProductEventTypeRecord::StateListed => {
                    ProductEventPayload::StateListed(ProductStateChangeEventPayload {
                        shop_id,
                        shops_product_id,
                        old_state: record.old_state.map(ProductState::from).ok_or(
                            MissingPersistenceField::new(field!(old_state@ProductEventRecord)),
                        )?,
                    })
                }
                ProductEventTypeRecord::StateAvailable => {
                    ProductEventPayload::StateAvailable(ProductStateChangeEventPayload {
                        shop_id,
                        shops_product_id,
                        old_state: record.old_state.map(ProductState::from).ok_or(
                            MissingPersistenceField::new(field!(old_state@ProductEventRecord)),
                        )?,
                    })
                }
                ProductEventTypeRecord::StateReserved => {
                    ProductEventPayload::StateReserved(ProductStateChangeEventPayload {
                        shop_id,
                        shops_product_id,
                        old_state: record.old_state.map(ProductState::from).ok_or(
                            MissingPersistenceField::new(field!(old_state@ProductEventRecord)),
                        )?,
                    })
                }
                ProductEventTypeRecord::StateSold => {
                    ProductEventPayload::StateSold(ProductStateChangeEventPayload {
                        shop_id,
                        shops_product_id,
                        old_state: record.old_state.map(ProductState::from).ok_or(
                            MissingPersistenceField::new(field!(old_state@ProductEventRecord)),
                        )?,
                    })
                }
                ProductEventTypeRecord::StateRemoved => {
                    ProductEventPayload::StateRemoved(ProductStateChangeEventPayload {
                        shop_id,
                        shops_product_id,
                        old_state: record.old_state.map(ProductState::from).ok_or(
                            MissingPersistenceField::new(field!(old_state@ProductEventRecord)),
                        )?,
                    })
                }
                ProductEventTypeRecord::StateUnknown => {
                    ProductEventPayload::StateUnknown(ProductStateChangeEventPayload {
                        shop_id,
                        shops_product_id,
                        old_state: record.old_state.map(ProductState::from).ok_or(
                            MissingPersistenceField::new(field!(old_state@ProductEventRecord)),
                        )?,
                    })
                }
                ProductEventTypeRecord::PriceDiscovered => {
                    ProductEventPayload::PriceDiscovered(ProductPriceDiscoveryEventPayload {
                        shop_id,
                        shops_product_id,
                        native_price: record.new_price_native.map(Price::from).ok_or(
                            MissingPersistenceField::new(
                                field!(new_price_native@ProductEventRecord),
                            ),
                        )?,
                        other_price: new_other_price,
                    })
                }
                ProductEventTypeRecord::PriceDropped => {
                    ProductEventPayload::PriceDropped(ProductPriceChangeEventPayload {
                        shop_id,
                        shops_product_id,
                        new_native_price: record.new_price_native.map(Price::from).ok_or(
                            MissingPersistenceField::new(
                                field!(new_price_native@ProductEventRecord),
                            ),
                        )?,
                        new_other_price,
                        old_native_price: record.old_price_native.map(Price::from).ok_or(
                            MissingPersistenceField::new(
                                field!(old_price_native@ProductEventRecord),
                            ),
                        )?,
                        old_other_price,
                    })
                }
                ProductEventTypeRecord::PriceIncreased => {
                    ProductEventPayload::PriceIncreased(ProductPriceChangeEventPayload {
                        shop_id,
                        shops_product_id,
                        new_native_price: record.new_price_native.map(Price::from).ok_or(
                            MissingPersistenceField::new(
                                field!(new_price_native@ProductEventRecord),
                            ),
                        )?,
                        new_other_price,
                        old_native_price: record.old_price_native.map(Price::from).ok_or(
                            MissingPersistenceField::new(
                                field!(old_price_native@ProductEventRecord),
                            ),
                        )?,
                        old_other_price,
                    })
                }
                ProductEventTypeRecord::PriceRemoved => {
                    ProductEventPayload::PriceRemoved(ProductPriceRemovedEventPayload {
                        shop_id,
                        shops_product_id,
                        old_native_price: record.old_price_native.map(Price::from).ok_or(
                            MissingPersistenceField::new(
                                field!(old_price_native@ProductEventRecord),
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

    impl Dummy<Faker> for ProductEventRecord {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config
                .fake_with_rng::<ProductEvent, _>(rng)
                .try_into()
                .unwrap()
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::dynamodb::product_event_record::ProductEventRecord;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_get_product_event_record() {
            let _ = Faker.fake::<ProductEventRecord>();
        }
    }
}
