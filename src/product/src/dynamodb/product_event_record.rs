use crate::core::product_event::{
    ItemCommonEventPayload, ItemCreatedEventPayload, ItemEventPayload, ItemPriceChangeEventPayload,
    ItemPriceDiscoveryEventPayload, ItemPriceRemovedEventPayload, ItemStateChangeEventPayload,
    ProductEvent,
};
use crate::dynamodb::product_event_type_record::ProductEventTypeRecord;
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
use std::collections::HashMap;
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, error};
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    pub title_native: Option<TextRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_de: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_en: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_native: Option<TextRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_de: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_en: Option<String>,

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
    pub images: Option<Vec<Url>>,

    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

pub fn mk_pk(shop_id: &ShopId, shops_product_id: &ShopsProductId) -> String {
    format!("item#shop_id#{shop_id}#shops_product_id#{shops_product_id}")
}

pub fn mk_sk(timestamp: &OffsetDateTime) -> Result<String, error::Format> {
    Ok(format!("item#event#{}", timestamp.format(&Rfc3339)?))
}

impl ProductEventRecord {
    pub fn into_item_key(self) -> ProductKey {
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
            ItemEventPayload::Created(payload) => {
                let mut payload = payload;
                payload.other_title.insert(
                    payload.native_title.localization,
                    payload.native_title.payload.clone(),
                );

                let title_de = payload.other_title.remove(&Language::De).map(String::from);
                let title_en = payload.other_title.remove(&Language::En).map(String::from);

                if let Some(description_native) = payload.native_description.as_ref() {
                    payload.other_description.insert(
                        description_native.localization,
                        description_native.payload.clone(),
                    );
                }
                let description_de = payload
                    .other_description
                    .remove(&Language::De)
                    .map(String::from);
                let description_en = payload
                    .other_description
                    .remove(&Language::En)
                    .map(String::from);

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
                    title_native: Some(payload.native_title.into()),
                    title_de,
                    title_en,
                    description_native: payload.native_description.map(TextRecord::from),
                    description_de,
                    description_en,
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
                    images: Some(payload.images),
                    timestamp: domain.timestamp,
                };
                Ok(record)
            }
            ItemEventPayload::StateListed(payload) => Ok(mk_state_event_record(
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
            ItemEventPayload::StateReserved(payload) => Ok(mk_state_event_record(
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
            ItemEventPayload::StateAvailable(payload) => Ok(mk_state_event_record(
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
            ItemEventPayload::StateSold(payload) => Ok(mk_state_event_record(
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
            ItemEventPayload::StateRemoved(payload) => Ok(mk_state_event_record(
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
            ItemEventPayload::StateUnknown(payload) => Ok(mk_state_event_record(
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
            ItemEventPayload::PriceDiscovered(payload) => Ok(ProductEventRecord {
                pk,
                sk,
                product_id,
                event_id,
                event_type,
                event_type_schema_version: 0,
                shop_id,
                shops_product_id,
                shop_name: None,
                title_native: None,
                title_de: None,
                title_en: None,
                description_native: None,
                description_de: None,
                description_en: None,
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
                timestamp: domain.timestamp,
            }),
            ItemEventPayload::PriceIncreased(payload) => Ok(mk_price_change_event_record(
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
            ItemEventPayload::PriceDropped(payload) => Ok(mk_price_change_event_record(
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
            ItemEventPayload::PriceRemoved(payload) => Ok(ProductEventRecord {
                pk,
                sk,
                product_id,
                event_id,
                event_type,
                event_type_schema_version: 0,
                shop_id,
                shops_product_id,
                shop_name: None,
                title_native: None,
                title_de: None,
                title_en: None,
                description_native: None,
                description_de: None,
                description_en: None,
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
                timestamp: domain.timestamp,
            }),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn mk_state_event_record(
    new_item_state_record: ProductStateRecord,
    old_item_state_record: ProductStateRecord,
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
        title_native: None,
        title_de: None,
        title_en: None,
        description_native: None,
        description_de: None,
        description_en: None,
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
        new_state: Some(new_item_state_record),
        old_state: Some(old_item_state_record),
        url: None,
        images: None,
        timestamp,
    }
}

#[allow(clippy::too_many_arguments)]
fn mk_price_change_event_record(
    item_price_change_event_payload: ItemPriceChangeEventPayload,
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
        title_native: None,
        title_de: None,
        title_en: None,
        description_native: None,
        description_de: None,
        description_en: None,
        new_price_native: Some(item_price_change_event_payload.new_native_price.into()),
        new_price_eur: item_price_change_event_payload
            .new_other_price
            .get(&Currency::Eur)
            .copied()
            .map(u64::from),
        new_price_usd: item_price_change_event_payload
            .new_other_price
            .get(&Currency::Usd)
            .copied()
            .map(u64::from),
        new_price_gbp: item_price_change_event_payload
            .new_other_price
            .get(&Currency::Gbp)
            .copied()
            .map(u64::from),
        new_price_aud: item_price_change_event_payload
            .new_other_price
            .get(&Currency::Aud)
            .copied()
            .map(u64::from),
        new_price_cad: item_price_change_event_payload
            .new_other_price
            .get(&Currency::Cad)
            .copied()
            .map(u64::from),
        new_price_nzd: item_price_change_event_payload
            .new_other_price
            .get(&Currency::Nzd)
            .copied()
            .map(u64::from),
        old_price_native: Some(item_price_change_event_payload.old_native_price.into()),
        old_price_eur: item_price_change_event_payload
            .old_other_price
            .get(&Currency::Eur)
            .copied()
            .map(u64::from),
        old_price_usd: item_price_change_event_payload
            .old_other_price
            .get(&Currency::Usd)
            .copied()
            .map(u64::from),
        old_price_gbp: item_price_change_event_payload
            .old_other_price
            .get(&Currency::Gbp)
            .copied()
            .map(u64::from),
        old_price_aud: item_price_change_event_payload
            .old_other_price
            .get(&Currency::Aud)
            .copied()
            .map(u64::from),
        old_price_cad: item_price_change_event_payload
            .old_other_price
            .get(&Currency::Cad)
            .copied()
            .map(u64::from),
        old_price_nzd: item_price_change_event_payload
            .old_other_price
            .get(&Currency::Nzd)
            .copied()
            .map(u64::from),
        new_state: None,
        old_state: None,
        url: None,
        images: None,
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
                    let mut other_title = HashMap::with_capacity(2);
                    if let Some(title_de) = record.title_de {
                        other_title.insert(Language::De, title_de.into());
                    }
                    if let Some(title_en) = record.title_en {
                        other_title.insert(Language::En, title_en.into());
                    }

                    let mut other_description = HashMap::with_capacity(2);
                    if let Some(description_de) = record.description_de {
                        other_description.insert(Language::De, description_de.into());
                    }
                    if let Some(description_en) = record.description_en {
                        other_description.insert(Language::En, description_en.into());
                    }

                    ItemEventPayload::Created(ItemCreatedEventPayload {
                        shop_id,
                        shops_product_id,
                        shop_name: record.shop_name.map(ShopName::from).ok_or(
                            MissingPersistenceField::new(field!(shop_name@ProductEventRecord)),
                        )?,
                        native_title: record.title_native.map(Localized::from).ok_or(
                            MissingPersistenceField::new(field!(title_native@ProductEventRecord)),
                        )?,
                        other_title,
                        native_description: record.description_native.map(Localized::from),
                        other_description,
                        native_price: record.new_price_native.map(Price::from),
                        other_price: new_other_price,
                        state: record.new_state.map(ProductState::from).ok_or(
                            MissingPersistenceField::new(field!(new_state@ProductEventRecord)),
                        )?,
                        url: record
                            .url
                            .ok_or(MissingPersistenceField::new(field!(url@ProductEventRecord)))?,
                        images: record.images.unwrap_or_default(),
                    })
                }
                ProductEventTypeRecord::StateListed => {
                    ItemEventPayload::StateListed(ItemStateChangeEventPayload {
                        shop_id,
                        shops_product_id,
                        old_state: record.old_state.map(ProductState::from).ok_or(
                            MissingPersistenceField::new(field!(old_state@ProductEventRecord)),
                        )?,
                    })
                }
                ProductEventTypeRecord::StateAvailable => {
                    ItemEventPayload::StateAvailable(ItemStateChangeEventPayload {
                        shop_id,
                        shops_product_id,
                        old_state: record.old_state.map(ProductState::from).ok_or(
                            MissingPersistenceField::new(field!(old_state@ProductEventRecord)),
                        )?,
                    })
                }
                ProductEventTypeRecord::StateReserved => {
                    ItemEventPayload::StateReserved(ItemStateChangeEventPayload {
                        shop_id,
                        shops_product_id,
                        old_state: record.old_state.map(ProductState::from).ok_or(
                            MissingPersistenceField::new(field!(old_state@ProductEventRecord)),
                        )?,
                    })
                }
                ProductEventTypeRecord::StateSold => {
                    ItemEventPayload::StateSold(ItemStateChangeEventPayload {
                        shop_id,
                        shops_product_id,
                        old_state: record.old_state.map(ProductState::from).ok_or(
                            MissingPersistenceField::new(field!(old_state@ProductEventRecord)),
                        )?,
                    })
                }
                ProductEventTypeRecord::StateRemoved => {
                    ItemEventPayload::StateRemoved(ItemStateChangeEventPayload {
                        shop_id,
                        shops_product_id,
                        old_state: record.old_state.map(ProductState::from).ok_or(
                            MissingPersistenceField::new(field!(old_state@ProductEventRecord)),
                        )?,
                    })
                }
                ProductEventTypeRecord::StateUnknown => {
                    ItemEventPayload::StateUnknown(ItemStateChangeEventPayload {
                        shop_id,
                        shops_product_id,
                        old_state: record.old_state.map(ProductState::from).ok_or(
                            MissingPersistenceField::new(field!(old_state@ProductEventRecord)),
                        )?,
                    })
                }
                ProductEventTypeRecord::PriceDiscovered => {
                    ItemEventPayload::PriceDiscovered(ItemPriceDiscoveryEventPayload {
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
                    ItemEventPayload::PriceDropped(ItemPriceChangeEventPayload {
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
                    ItemEventPayload::PriceIncreased(ItemPriceChangeEventPayload {
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
                    ItemEventPayload::PriceRemoved(ItemPriceRemovedEventPayload {
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
        fn should_fake_get_item_event_record() {
            let _ = Faker.fake::<ProductEventRecord>();
        }
    }
}
