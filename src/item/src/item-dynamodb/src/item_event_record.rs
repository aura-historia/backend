use crate::item_event_type_record::ItemEventTypeRecord;
use crate::item_state_record::ItemStateRecord;
use common::currency::domain::Currency;
use common::error::missing_field::MissingPersistenceField;
use common::event::Event;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::item_id::{ItemId, ItemKey};
use common::item_state::domain::ItemState;
use common::language::domain::Language;
use common::language::record::TextRecord;
use common::localized::Localized;
use common::price::domain::Price;
use common::price::record::PriceRecord;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shops_item_id::ShopsItemId;
use field::field;
use item_core::hash::ItemHash;
use item_core::item_event::{
    ItemCommonEventPayload, ItemCreatedEventPayload, ItemEvent, ItemEventPayload,
    ItemPriceChangeEventPayload, ItemStateChangeEventPayload,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, error};
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemEventRecord {
    pub pk: String,

    pub sk: String,

    pub item_id: ItemId,

    pub event_id: EventId,

    pub event_type: ItemEventTypeRecord,

    pub shop_id: ShopId,

    pub shops_item_id: ShopsItemId,

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
    pub state: Option<ItemStateRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<Url>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub images: Option<Vec<Url>>,

    pub hash: ItemHash,

    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

impl ItemEventRecord {
    pub fn into_item_key(self) -> ItemKey {
        ItemKey::new(self.shop_id, self.shops_item_id)
    }
}

impl HasKey for ItemEventRecord {
    type Key = ItemKey;

    fn key(&self) -> ItemKey {
        ItemKey {
            shop_id: self.shop_id,
            shops_item_id: self.shops_item_id.clone(),
        }
    }
}

impl TryFrom<ItemEvent> for ItemEventRecord {
    type Error = error::Format;
    fn try_from(domain: ItemEvent) -> Result<Self, Self::Error> {
        let shop_id = *domain.payload.shop_id();
        let shops_item_id = domain.payload.shops_item_id();
        let pk = format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id}");
        let sk = format!("item#event#{}", domain.timestamp.format(&Rfc3339)?);
        let item_id = domain.aggregate_id;
        let event_id = domain.event_id;
        let event_type: ItemEventTypeRecord = (&domain.payload).into();
        let shops_item_id = shops_item_id.clone();

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

                let record = ItemEventRecord {
                    pk,
                    sk,
                    item_id,
                    event_id,
                    event_type,
                    shop_id,
                    shops_item_id,
                    shop_name: Some(payload.shop_name.into()),
                    title_native: Some(payload.native_title.into()),
                    title_de,
                    title_en,
                    description_native: payload.native_description.map(TextRecord::from),
                    description_de,
                    description_en,
                    price_native: payload.native_price.map(PriceRecord::from),
                    price_eur: payload
                        .other_price
                        .get(&Currency::Eur)
                        .copied()
                        .map(u64::from),
                    price_usd: payload
                        .other_price
                        .get(&Currency::Usd)
                        .copied()
                        .map(u64::from),
                    price_gbp: payload
                        .other_price
                        .get(&Currency::Gbp)
                        .copied()
                        .map(u64::from),
                    price_aud: payload
                        .other_price
                        .get(&Currency::Aud)
                        .copied()
                        .map(u64::from),
                    price_cad: payload
                        .other_price
                        .get(&Currency::Cad)
                        .copied()
                        .map(u64::from),
                    price_nzd: payload
                        .other_price
                        .get(&Currency::Nzd)
                        .copied()
                        .map(u64::from),
                    state: Some(payload.state.into()),
                    url: Some(payload.url),
                    images: Some(payload.images),
                    hash: payload.hash,
                    timestamp: domain.timestamp,
                };
                Ok(record)
            }
            ItemEventPayload::StateListed(payload) => Ok(mk_state_event_record(
                ItemStateRecord::Listed,
                payload,
                pk,
                sk,
                item_id,
                event_id,
                event_type,
                shop_id,
                shops_item_id,
                domain.timestamp,
            )),
            ItemEventPayload::StateReserved(payload) => Ok(mk_state_event_record(
                ItemStateRecord::Reserved,
                payload,
                pk,
                sk,
                item_id,
                event_id,
                event_type,
                shop_id,
                shops_item_id,
                domain.timestamp,
            )),
            ItemEventPayload::StateAvailable(payload) => Ok(mk_state_event_record(
                ItemStateRecord::Available,
                payload,
                pk,
                sk,
                item_id,
                event_id,
                event_type,
                shop_id,
                shops_item_id,
                domain.timestamp,
            )),
            ItemEventPayload::StateSold(payload) => Ok(mk_state_event_record(
                ItemStateRecord::Sold,
                payload,
                pk,
                sk,
                item_id,
                event_id,
                event_type,
                shop_id,
                shops_item_id,
                domain.timestamp,
            )),
            ItemEventPayload::StateRemoved(payload) => Ok(mk_state_event_record(
                ItemStateRecord::Removed,
                payload,
                pk,
                sk,
                item_id,
                event_id,
                event_type,
                shop_id,
                shops_item_id,
                domain.timestamp,
            )),
            ItemEventPayload::StateUnknown(payload) => Ok(mk_state_event_record(
                ItemStateRecord::Unknown,
                payload,
                pk,
                sk,
                item_id,
                event_id,
                event_type,
                shop_id,
                shops_item_id,
                domain.timestamp,
            )),
            ItemEventPayload::PriceDiscovered(payload) => Ok(mk_price_event_record(
                payload,
                pk,
                sk,
                item_id,
                event_id,
                event_type,
                shop_id,
                shops_item_id,
                domain.timestamp,
            )),
            ItemEventPayload::PriceIncreased(payload) => Ok(mk_price_event_record(
                payload,
                pk,
                sk,
                item_id,
                event_id,
                event_type,
                shop_id,
                shops_item_id,
                domain.timestamp,
            )),
            ItemEventPayload::PriceDropped(payload) => Ok(mk_price_event_record(
                payload,
                pk,
                sk,
                item_id,
                event_id,
                event_type,
                shop_id,
                shops_item_id,
                domain.timestamp,
            )),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn mk_state_event_record(
    item_state_record: ItemStateRecord,
    item_state_change_event_payload: ItemStateChangeEventPayload,
    pk: String,
    sk: String,
    item_id: ItemId,
    event_id: EventId,
    event_type: ItemEventTypeRecord,
    shop_id: ShopId,
    shops_item_id: ShopsItemId,
    timestamp: OffsetDateTime,
) -> ItemEventRecord {
    ItemEventRecord {
        pk,
        sk,
        item_id,
        event_id,
        event_type,
        shop_id,
        shops_item_id,
        shop_name: None,
        title_native: None,
        title_de: None,
        title_en: None,
        description_native: None,
        description_de: None,
        description_en: None,
        price_native: None,
        price_eur: None,
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: Some(item_state_record),
        url: None,
        images: None,
        hash: item_state_change_event_payload.hash,
        timestamp,
    }
}

#[allow(clippy::too_many_arguments)]
fn mk_price_event_record(
    item_price_change_event_payload: ItemPriceChangeEventPayload,
    pk: String,
    sk: String,
    item_id: ItemId,
    event_id: EventId,
    event_type: ItemEventTypeRecord,
    shop_id: ShopId,
    shops_item_id: ShopsItemId,
    timestamp: OffsetDateTime,
) -> ItemEventRecord {
    ItemEventRecord {
        pk,
        sk,
        item_id,
        event_id,
        event_type,
        shop_id,
        shops_item_id,
        shop_name: None,
        title_native: None,
        title_de: None,
        title_en: None,
        description_native: None,
        description_de: None,
        description_en: None,
        price_native: Some(item_price_change_event_payload.native_price.into()),
        price_eur: item_price_change_event_payload
            .other_price
            .get(&Currency::Eur)
            .copied()
            .map(u64::from),
        price_usd: item_price_change_event_payload
            .other_price
            .get(&Currency::Usd)
            .copied()
            .map(u64::from),
        price_gbp: item_price_change_event_payload
            .other_price
            .get(&Currency::Gbp)
            .copied()
            .map(u64::from),
        price_aud: item_price_change_event_payload
            .other_price
            .get(&Currency::Aud)
            .copied()
            .map(u64::from),
        price_cad: item_price_change_event_payload
            .other_price
            .get(&Currency::Cad)
            .copied()
            .map(u64::from),
        price_nzd: item_price_change_event_payload
            .other_price
            .get(&Currency::Nzd)
            .copied()
            .map(u64::from),
        state: None,
        url: None,
        images: None,
        hash: item_price_change_event_payload.hash,
        timestamp,
    }
}

impl TryFrom<ItemEventRecord> for ItemEvent {
    type Error = MissingPersistenceField;

    fn try_from(record: ItemEventRecord) -> Result<Self, Self::Error> {
        let shop_id = record.shop_id;
        let shops_item_id = record.shops_item_id;
        let hash = record.hash;
        let mut other_price = HashMap::with_capacity(6);
        if let Some(amount_eur) = record.price_eur {
            other_price.insert(Currency::Eur, amount_eur.into());
        }
        if let Some(amount_gbp) = record.price_gbp {
            other_price.insert(Currency::Gbp, amount_gbp.into());
        }
        if let Some(amount_usd) = record.price_usd {
            other_price.insert(Currency::Usd, amount_usd.into());
        }
        if let Some(amount_aud) = record.price_aud {
            other_price.insert(Currency::Aud, amount_aud.into());
        }
        if let Some(amount_cad) = record.price_cad {
            other_price.insert(Currency::Cad, amount_cad.into());
        }
        if let Some(amount_nzd) = record.price_nzd {
            other_price.insert(Currency::Nzd, amount_nzd.into());
        }

        let event = Event {
            aggregate_id: record.item_id,
            event_id: record.event_id,
            timestamp: record.timestamp,
            payload: match record.event_type {
                ItemEventTypeRecord::Created => {
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
                        shops_item_id,
                        shop_name: record.shop_name.map(ShopName::from).ok_or(
                            MissingPersistenceField::new(field!(shop_name@ItemEventRecord)),
                        )?,
                        native_title: record.title_native.map(Localized::from).ok_or(
                            MissingPersistenceField::new(field!(title_native@ItemEventRecord)),
                        )?,
                        other_title,
                        native_description: record.description_native.map(Localized::from),
                        other_description,
                        native_price: record.price_native.map(Price::from),
                        other_price,
                        state: record
                            .state
                            .map(ItemState::from)
                            .ok_or(MissingPersistenceField::new(field!(state@ItemEventRecord)))?,
                        url: record
                            .url
                            .ok_or(MissingPersistenceField::new(field!(url@ItemEventRecord)))?,
                        images: record.images.unwrap_or_default(),
                        hash,
                    })
                }
                ItemEventTypeRecord::StateListed => {
                    ItemEventPayload::StateListed(ItemStateChangeEventPayload {
                        shop_id,
                        shops_item_id,
                        hash,
                    })
                }
                ItemEventTypeRecord::StateAvailable => {
                    ItemEventPayload::StateAvailable(ItemStateChangeEventPayload {
                        shop_id,
                        shops_item_id,
                        hash,
                    })
                }
                ItemEventTypeRecord::StateReserved => {
                    ItemEventPayload::StateReserved(ItemStateChangeEventPayload {
                        shop_id,
                        shops_item_id,
                        hash,
                    })
                }
                ItemEventTypeRecord::StateSold => {
                    ItemEventPayload::StateSold(ItemStateChangeEventPayload {
                        shop_id,
                        shops_item_id,
                        hash,
                    })
                }
                ItemEventTypeRecord::StateRemoved => {
                    ItemEventPayload::StateRemoved(ItemStateChangeEventPayload {
                        shop_id,
                        shops_item_id,
                        hash,
                    })
                }
                ItemEventTypeRecord::StateUnknown => {
                    ItemEventPayload::StateUnknown(ItemStateChangeEventPayload {
                        shop_id,
                        shops_item_id,
                        hash,
                    })
                }
                ItemEventTypeRecord::PriceDiscovered => {
                    ItemEventPayload::PriceDiscovered(ItemPriceChangeEventPayload {
                        shop_id,
                        shops_item_id,
                        native_price: record.price_native.map(Price::from).ok_or(
                            MissingPersistenceField::new(field!(price_native@ItemEventRecord)),
                        )?,
                        other_price,
                        hash,
                    })
                }
                ItemEventTypeRecord::PriceDropped => {
                    ItemEventPayload::PriceDropped(ItemPriceChangeEventPayload {
                        shop_id,
                        shops_item_id,
                        native_price: record.price_native.map(Price::from).ok_or(
                            MissingPersistenceField::new(field!(price_native@ItemEventRecord)),
                        )?,
                        other_price,
                        hash,
                    })
                }
                ItemEventTypeRecord::PriceIncreased => {
                    ItemEventPayload::PriceIncreased(ItemPriceChangeEventPayload {
                        shop_id,
                        shops_item_id,
                        native_price: record.price_native.map(Price::from).ok_or(
                            MissingPersistenceField::new(field!(price_native@ItemEventRecord)),
                        )?,
                        other_price,
                        hash,
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

    impl Dummy<Faker> for ItemEventRecord {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config
                .fake_with_rng::<ItemEvent, _>(rng)
                .try_into()
                .unwrap()
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::item_event_record::ItemEventRecord;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_get_item_event_record() {
            let _ = Faker.fake::<ItemEventRecord>();
        }
    }
}
