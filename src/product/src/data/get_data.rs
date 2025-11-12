use crate::core::product::LocalizedItemView;
use crate::data::get_product_event_data::GetProductEventData;
use crate::data::product_state_data::ProductStateData;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::language::data::LocalizedTextData;
use common::price::data::PriceData;
use common::product_id::{ProductId, ProductKey};
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetItemData {
    pub product_id: ProductId,

    pub event_id: EventId,

    pub shop_id: ShopId,

    pub shops_product_id: ShopsProductId,

    pub shop_name: String,

    pub title: LocalizedTextData,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<LocalizedTextData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceData>,

    pub state: ProductStateData,

    pub url: Url,

    #[serde(default)]
    pub images: Vec<Url>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub history: Option<Vec<GetProductEventData>>,
}

impl HasKey for GetItemData {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        ProductKey {
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id.clone(),
        }
    }
}

impl From<LocalizedItemView> for GetItemData {
    fn from(item_view: LocalizedItemView) -> Self {
        GetItemData {
            product_id: item_view.product_id,
            event_id: item_view.event_id,
            shop_id: item_view.shop_id,
            shops_product_id: item_view.shops_product_id,
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
            GetProductEventData, ItemEventPayloadData, ItemEventPriceChangedPayloadData,
            ItemEventStateChangedPayloadData, ItemEventTypeData,
        },
        data::item_state_data::ProductStateData,
    };
    use common::{
        currency::data::CurrencyData,
        event_id::EventId,
        language::data::{LanguageData, LocalizedTextData},
        price::data::PriceData,
        product_id::ProductId,
        shop_id::ShopId,
        shops_product_id::ShopsProductId,
    };
    use serde_json::json;
    use time::macros::utc_datetime;
    use url::Url;

    #[test]
    fn should_serialize_get_item_data() {
        let product_id = ProductId::new();
        let event_id = EventId::new();
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let dto = GetItemData {
            product_id,
            event_id,
            shop_id,
            shops_product_id: shops_product_id.clone(),
            shop_name: "My shop".into(),
            title: LocalizedTextData::new("Mein titel", LanguageData::De),
            description: Some(LocalizedTextData::new("My description", LanguageData::En)),
            price: Some(PriceData::new(CurrencyData::Eur, 50000)),
            state: ProductStateData::Reserved,
            url: Url::parse("https://my-shop.de/item").unwrap(),
            images: vec![
                Url::parse("https://my-shop.de/item/images/1").unwrap(),
                Url::parse("https://my-shop.de/item/images/2").unwrap(),
            ],
            created: utc_datetime!(2025 - 05 - 05 0:00).into(),
            updated: utc_datetime!(2025 - 05 - 05 0:00).into(),
            history: Some(vec![
                GetProductEventData {
                    event_type: ItemEventTypeData::StateAvailable,
                    product_id,
                    event_id,
                    shop_id,
                    shops_product_id: shops_product_id.clone(),
                    payload: ItemEventPayloadData::StateAvailable(
                        ItemEventStateChangedPayloadData {
                            old_state: ProductStateData::Listed,
                            new_state: ProductStateData::Available,
                        },
                    ),
                    timestamp: utc_datetime!(2025 - 05 - 05 0:00).into(),
                },
                GetProductEventData {
                    event_type: ItemEventTypeData::PriceDropped,
                    product_id,
                    event_id,
                    shop_id,
                    shops_product_id: shops_product_id.clone(),
                    payload: ItemEventPayloadData::PriceDropped(ItemEventPriceChangedPayloadData {
                        old_price: PriceData::new(CurrencyData::Eur, 69),
                        new_price: PriceData::new(CurrencyData::Eur, 42),
                    }),
                    timestamp: utc_datetime!(2025 - 05 - 05 0:00).into(),
                },
            ]),
        };

        let expected = json!({
            "productId": product_id,
            "eventId": event_id,
            "shopId": shop_id,
            "shopsProductId": shops_product_id,
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
                    "productId": product_id,
                    "eventId": event_id,
                    "shopId": shop_id,
                    "shopsProductId": shops_product_id,
                    "payload": {
                        "oldState": "LISTED",
                        "newState": "AVAILABLE"
                    },
                    "timestamp": "2025-05-05T00:00:00Z",
                },
                {
                    "eventType": "PRICE_DROPPED",
                    "productId": product_id,
                    "eventId": event_id,
                    "shopId": shop_id,
                    "shopsProductId": shops_product_id,
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
