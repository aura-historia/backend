use crate::core::item::LocalizedItemView;
use crate::data::get_item_event_data::GetItemEventData;
use crate::data::item_state_data::ItemStateData;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::item_id::{ItemId, ItemKey};
use common::language::data::LocalizedTextData;
use common::price::data::PriceData;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetItemData {
    pub item_id: ItemId,

    pub event_id: EventId,

    pub shop_id: ShopId,

    pub shops_item_id: ShopsItemId,

    pub shop_name: String,

    pub title: LocalizedTextData,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<LocalizedTextData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceData>,

    pub state: ItemStateData,

    pub url: Url,

    #[serde(default)]
    pub images: Vec<Url>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub history: Option<Vec<GetItemEventData>>,
}

impl HasKey for GetItemData {
    type Key = ItemKey;

    fn key(&self) -> Self::Key {
        ItemKey {
            shop_id: self.shop_id,
            shops_item_id: self.shops_item_id.clone(),
        }
    }
}

impl From<LocalizedItemView> for GetItemData {
    fn from(item_view: LocalizedItemView) -> Self {
        GetItemData {
            item_id: item_view.item_id,
            event_id: item_view.event_id,
            shop_id: item_view.shop_id,
            shops_item_id: item_view.shops_item_id,
            shop_name: item_view.shop_name.into(),
            title: item_view.title.into(),
            description: item_view.description.map(LocalizedTextData::from),
            price: item_view.price.map(PriceData::from),
            state: item_view.state.into(),
            url: item_view.url,
            images: item_view.images,
            created: item_view.created,
            updated: item_view.updated,
            history: item_view
                .history
                .map(|events| events.into_iter().map(|event| event.into()).collect()),
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for GetItemData {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config.fake_with_rng::<LocalizedItemView, _>(rng).into()
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::data::get_data::GetItemData;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_get_item_data() {
            let _ = Faker.fake::<GetItemData>();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        data::get_data::GetItemData,
        data::get_item_event_data::{
            GetItemEventData, ItemEventPayloadData, ItemEventPriceChangedPayloadData,
            ItemEventStateChangedPayloadData, ItemEventTypeData,
        },
        data::item_state_data::ItemStateData,
    };
    use common::{
        currency::data::CurrencyData,
        event_id::EventId,
        item_id::ItemId,
        language::data::{LanguageData, LocalizedTextData},
        price::data::PriceData,
        shop_id::ShopId,
        shops_item_id::ShopsItemId,
    };
    use serde_json::json;
    use time::macros::utc_datetime;
    use url::Url;

    #[test]
    fn should_serialize_get_item_data() {
        let item_id = ItemId::new();
        let event_id = EventId::new();
        let shop_id = ShopId::new();
        let shops_item_id = ShopsItemId::new();
        let dto = GetItemData {
            item_id,
            event_id,
            shop_id,
            shops_item_id: shops_item_id.clone(),
            shop_name: "My shop".into(),
            title: LocalizedTextData::new("Mein titel", LanguageData::De),
            description: Some(LocalizedTextData::new("My description", LanguageData::En)),
            price: Some(PriceData::new(CurrencyData::Eur, 50000)),
            state: ItemStateData::Reserved,
            url: Url::parse("https://my-shop.de/item").unwrap(),
            images: vec![
                Url::parse("https://my-shop.de/item/images/1").unwrap(),
                Url::parse("https://my-shop.de/item/images/2").unwrap(),
            ],
            created: utc_datetime!(2025 - 05 - 05 0:00).into(),
            updated: utc_datetime!(2025 - 05 - 05 0:00).into(),
            history: Some(vec![
                GetItemEventData {
                    event_type: ItemEventTypeData::StateAvailable,
                    item_id,
                    event_id,
                    shop_id,
                    shops_item_id: shops_item_id.clone(),
                    payload: ItemEventPayloadData::StateAvailable(
                        ItemEventStateChangedPayloadData {
                            old_state: ItemStateData::Listed,
                            new_state: ItemStateData::Available,
                        },
                    ),
                    timestamp: utc_datetime!(2025 - 05 - 05 0:00).into(),
                },
                GetItemEventData {
                    event_type: ItemEventTypeData::PriceDropped,
                    item_id,
                    event_id,
                    shop_id,
                    shops_item_id: shops_item_id.clone(),
                    payload: ItemEventPayloadData::PriceDropped(ItemEventPriceChangedPayloadData {
                        old_price: PriceData::new(CurrencyData::Eur, 69),
                        new_price: PriceData::new(CurrencyData::Eur, 42),
                    }),
                    timestamp: utc_datetime!(2025 - 05 - 05 0:00).into(),
                },
            ]),
        };

        let expected = json!({
            "itemId": item_id,
            "eventId": event_id,
            "shopId": shop_id,
            "shopsItemId": shops_item_id,
            "shopName": "My shop",
            "title": {
                "text": "Mein titel",
                "language": "de"
            },
            "description": {
                "text": "My description",
                "language": "en"
            },
            "price": {
                "currency": "EUR",
                "amount": 50000
            },
            "state": "RESERVED",
            "url": "https://my-shop.de/item",
            "images": ["https://my-shop.de/item/images/1", "https://my-shop.de/item/images/2"],
            "created": "2025-05-05T00:00:00Z",
            "updated": "2025-05-05T00:00:00Z",
            "history": [
                {
                    "eventType": "STATE_AVAILABLE",
                    "itemId": item_id,
                    "eventId": event_id,
                    "shopId": shop_id,
                    "shopsItemId": shops_item_id,
                    "payload": {
                        "oldState": "LISTED",
                        "newState": "AVAILABLE"
                    },
                    "timestamp": "2025-05-05T00:00:00Z",
                },
                {
                    "eventType": "PRICE_DROPPED",
                    "itemId": item_id,
                    "eventId": event_id,
                    "shopId": shop_id,
                    "shopsItemId": shops_item_id,
                    "payload": {
                        "oldPrice": {
                            "amount": 69,
                            "currency": "EUR"
                        },
                        "newPrice": {
                            "amount": 42,
                            "currency": "EUR"
                        }
                    },
                    "timestamp": "2025-05-05T00:00:00Z",
                }
            ]
        });

        let actual = serde_json::to_value(dto).unwrap();
        assert_eq!(expected, actual);
    }
}
