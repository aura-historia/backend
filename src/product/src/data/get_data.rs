use crate::core::product::LocalizedProductView;
use crate::data::authenticity_data::AuthenticityData;
use crate::data::condition_data::ConditionData;
use crate::data::get_product_event_data::GetProductEventData;
use crate::data::product_image_data::ProductImageData;
use crate::data::product_state_data::ProductStateData;
use crate::data::provenance_data::ProvenanceData;
use crate::data::restoration_data::RestorationData;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::language::data::LocalizedTextData;
use common::price::data::PriceData;
use common::product_id::{ProductId, ProductKey};
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::year::Year;
use serde::{Deserialize, Serialize};
use shop::data::shop_type_data::ShopTypeData;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProductData {
    pub product_id: ProductId,

    pub event_id: EventId,

    pub shop_id: ShopId,

    pub shops_product_id: ShopsProductId,

    pub shop_name: String,

    pub shop_type: ShopTypeData,

    pub title: LocalizedTextData,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<LocalizedTextData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceData>,

    pub state: ProductStateData,

    pub url: Url,

    #[serde(default)]
    pub images: Vec<ProductImageData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_year_min: Option<Year>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_year: Option<Year>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_year_max: Option<Year>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub authenticity: Option<AuthenticityData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub condition: Option<ConditionData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub provenance: Option<ProvenanceData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub restoration: Option<RestorationData>,

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

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub history: Option<Vec<GetProductEventData>>,
}

impl HasKey for GetProductData {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        ProductKey {
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id.clone(),
        }
    }
}

impl From<LocalizedProductView> for GetProductData {
    fn from(product_view: LocalizedProductView) -> Self {
        GetProductData {
            product_id: product_view.product_id,
            event_id: product_view.event_id,
            shop_id: product_view.shop_id,
            shops_product_id: product_view.shops_product_id,
            shop_name: product_view.shop_name.into(),
            shop_type: product_view.shop_type.into(),
            title: product_view.title.into(),
            description: product_view.description.map(LocalizedTextData::from),
            price: product_view.price.map(PriceData::from),
            state: product_view.state.into(),
            url: product_view.url,
            images: product_view
                .images
                .into_iter()
                .map(ProductImageData::from)
                .collect(),
            origin_year_min: product_view.origin_year.and_then(|oy| oy.min()),
            origin_year: product_view.origin_year.and_then(|oy| oy.exact()),
            origin_year_max: product_view.origin_year.and_then(|oy| oy.max()),
            authenticity: product_view.authenticity.map(AuthenticityData::from),
            condition: product_view.condition.map(ConditionData::from),
            provenance: product_view.provenance.map(ProvenanceData::from),
            restoration: product_view.restoration.map(RestorationData::from),
            auction_start: product_view.auction_start,
            auction_end: product_view.auction_end,
            created: product_view.created,
            updated: product_view.updated,
            history: product_view
                .history
                .map(|events| events.into_iter().map(|event| event.into()).collect()),
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for GetProductData {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config.fake_with_rng::<LocalizedProductView, _>(rng).into()
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::data::get_data::GetProductData;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_get_product_data() {
            let _ = Faker.fake::<GetProductData>();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::data::{
        authenticity_data::AuthenticityData,
        condition_data::ConditionData,
        get_data::GetProductData,
        get_product_event_data::{
            GetProductEventData, ProductEventPayloadData, ProductEventPriceChangedPayloadData,
            ProductEventStateChangedPayloadData, ProductEventTypeData,
        },
        product_image_data::ProductImageData,
        product_state_data::ProductStateData,
        prohibited_content_data::ProhibitedContentData,
        provenance_data::ProvenanceData,
        restoration_data::RestorationData,
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
    use shop::data::shop_type_data::ShopTypeData;
    use time::macros::utc_datetime;
    use url::Url;

    #[test]
    fn should_serialize_get_product_data() {
        let product_id = ProductId::new();
        let event_id = EventId::new();
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let dto = GetProductData {
            product_id,
            event_id,
            shop_id,
            shops_product_id: shops_product_id.clone(),
            shop_name: "My shop".into(),
            shop_type: ShopTypeData::AuctionHouse,
            title: LocalizedTextData::new("Mein titel", LanguageData::De),
            description: Some(LocalizedTextData::new("My description", LanguageData::En)),
            price: Some(PriceData::new(CurrencyData::Eur, 50000)),
            state: ProductStateData::Reserved,
            url: Url::parse("https://my-shop.de/item").unwrap(),
            images: vec![
                ProductImageData {
                    url: Url::parse("https://my-shop.de/item/images/1").unwrap(),
                    prohibited_content: ProhibitedContentData::None,
                },
                ProductImageData {
                    url: Url::parse("https://my-shop.de/item/images/2").unwrap(),
                    prohibited_content: ProhibitedContentData::NaziGermany,
                },
            ],
            origin_year_min: Some(1900.into()),
            origin_year: Some(1900.into()),
            origin_year_max: Some(1903.into()),
            authenticity: Some(AuthenticityData::Original),
            condition: Some(ConditionData::Excellent),
            provenance: Some(ProvenanceData::Partial),
            restoration: Some(RestorationData::None),
            auction_start: None,
            auction_end: None,
            created: utc_datetime!(2025 - 05 - 05 0:00).into(),
            updated: utc_datetime!(2025 - 05 - 05 0:00).into(),
            history: Some(vec![
                GetProductEventData {
                    event_type: ProductEventTypeData::StateAvailable,
                    product_id,
                    event_id,
                    shop_id,
                    shops_product_id: shops_product_id.clone(),
                    payload: ProductEventPayloadData::StateAvailable(
                        ProductEventStateChangedPayloadData {
                            old_state: ProductStateData::Listed,
                            new_state: ProductStateData::Available,
                        },
                    ),
                    timestamp: utc_datetime!(2025 - 05 - 05 0:00).into(),
                },
                GetProductEventData {
                    event_type: ProductEventTypeData::PriceDropped,
                    product_id,
                    event_id,
                    shop_id,
                    shops_product_id: shops_product_id.clone(),
                    payload: ProductEventPayloadData::PriceDropped(
                        ProductEventPriceChangedPayloadData {
                            old_price: PriceData::new(CurrencyData::Eur, 69),
                            new_price: PriceData::new(CurrencyData::Eur, 42),
                        },
                    ),
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
            "shopType": "AUCTION_HOUSE",
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
            "images": [
                {
                    "url": "https://my-shop.de/item/images/1",
                    "prohibitedContent": "NONE"
                },
                {
                    "url": "https://my-shop.de/item/images/2",
                    "prohibitedContent": "NAZI_GERMANY"
                }
            ],
            "originYearMin": 1900,
            "originYear": 1900,
            "originYearMax": 1903,
            "authenticity": "ORIGINAL",
            "condition": "EXCELLENT",
            "provenance": "PARTIAL",
            "restoration": "NONE",
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
