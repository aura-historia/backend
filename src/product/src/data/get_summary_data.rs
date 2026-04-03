use crate::core::product::LocalizedProductView;
use crate::data::auction_data::AuctionData;
use crate::data::product_image_data::ProductImageData;
use crate::data::product_state_data::ProductStateData;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::language::data::LocalizedTextData;
use common::price::data::PriceData;
use common::product_id::{ProductId, ProductKey};
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::slug_id::SlugId;
use serde::{Deserialize, Serialize};
use shop::data::shop_type_data::ShopTypeData;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProductSummaryData {
    pub product_id: ProductId,
    pub product_slug_id: SlugId<6>,
    pub shop_slug_id: SlugId<0>,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: String,
    pub seller_name: String,
    pub shop_type: ShopTypeData,

    pub title: LocalizedTextData,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceData>,
    pub state: ProductStateData,
    pub url: Url,
    #[serde(default)]
    pub images: Vec<ProductImageData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub auction: Option<AuctionData>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl HasKey for GetProductSummaryData {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        ProductKey {
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id.clone(),
        }
    }
}

impl GetProductSummaryData {
    pub fn from_view(product_view: LocalizedProductView, prohibited_content_consent: bool) -> Self {
        GetProductSummaryData {
            product_id: product_view.product_id,
            product_slug_id: product_view.product_slug_id,
            shop_slug_id: product_view.shop_slug_id,
            event_id: product_view.event_id,
            shop_id: product_view.shop_id,
            shops_product_id: product_view.shops_product_id,
            shop_name: product_view.shop_name.into(),
            seller_name: product_view.seller_name.into(),
            shop_type: product_view.shop_type.into(),
            title: product_view.title.into(),
            price: product_view.price.map(PriceData::from),
            state: product_view.state.into(),
            url: product_view.url,
            images: product_view
                .images
                .into_iter()
                .map(|img| ProductImageData::from_with_consent(img, prohibited_content_consent))
                .collect(),
            auction: match (product_view.auction_start, product_view.auction_end) {
                (start, end @ Some(_)) => Some(AuctionData { start, end }),
                (start @ Some(_), end) => Some(AuctionData { start, end }),
                _ => None,
            },
            created: product_view.created,
            updated: product_view.updated,
        }
    }
}

impl From<LocalizedProductView> for GetProductSummaryData {
    fn from(product_view: LocalizedProductView) -> Self {
        GetProductSummaryData::from_view(product_view, false)
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for GetProductSummaryData {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config.fake_with_rng::<LocalizedProductView, _>(rng).into()
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::data::get_summary_data::GetProductSummaryData;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_get_product_data() {
            let _ = Faker.fake::<GetProductSummaryData>();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::data::{
        auction_data::AuctionData, get_summary_data::GetProductSummaryData,
        product_image_data::ProductImageData, product_state_data::ProductStateData,
        prohibited_content_data::ProhibitedContentData,
    };
    use common::{
        currency::data::CurrencyData,
        event_id::EventId,
        language::data::{LanguageData, LocalizedTextData},
        price::data::PriceData,
        product_id::ProductId,
        shop_id::ShopId,
        shops_product_id::ShopsProductId,
        slug_id::SlugId,
    };
    use serde_json::json;
    use shop::data::shop_type_data::ShopTypeData;
    use time::macros::utc_datetime;
    use url::Url;

    #[test]
    fn should_serialize_get_product_summary_data() {
        let product_id = ProductId::new();
        let event_id = EventId::new();
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let dto = GetProductSummaryData {
            product_id,
            product_slug_id: SlugId::raw("beedel-beep-bap-fa87c45d"),
            shop_slug_id: "my-shop".into(),
            event_id,
            shop_id,
            shops_product_id: shops_product_id.clone(),
            shop_name: "My shop".into(),
            seller_name: "My seller".into(),
            shop_type: ShopTypeData::AuctionHouse,
            title: LocalizedTextData::new("Mein titel", LanguageData::De),
            price: Some(PriceData::new(CurrencyData::Eur, 50000)),
            state: ProductStateData::Reserved,
            url: Url::parse("https://my-shop.de/item").unwrap(),
            images: vec![
                ProductImageData {
                    url: Some(Url::parse("https://my-shop.de/item/images/1").unwrap()),
                    prohibited_content: ProhibitedContentData::None,
                },
                ProductImageData {
                    url: Some(Url::parse("https://my-shop.de/item/images/2").unwrap()),
                    prohibited_content: ProhibitedContentData::NaziGermany,
                },
            ],
            auction: Some(AuctionData {
                start: Some(utc_datetime!(2025 - 05 - 01 12:00).into()),
                end: Some(utc_datetime!(2025 - 05 - 10 12:00).into()),
            }),
            created: utc_datetime!(2025 - 05 - 05 0:00).into(),
            updated: utc_datetime!(2025 - 05 - 05 0:00).into(),
        };

        let expected = json!({
            "productId": product_id,
            "productSlugId": "beedel-beep-bap-fa87c45d",
            "shopSlugId": "my-shop",
            "eventId": event_id,
            "shopId": shop_id,
            "shopsProductId": shops_product_id,
            "shopName": "My shop",
            "sellerName": "My seller",
            "shopType": "AUCTION_HOUSE",
            "title": {
                "text": "Mein titel",
                "language": "de"
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
            "auction": {
                "start": "2025-05-01T12:00:00Z",
                "end": "2025-05-10T12:00:00Z"
            },
            "created": "2025-05-05T00:00:00Z",
            "updated": "2025-05-05T00:00:00Z",
        });

        let actual = serde_json::to_value(dto).unwrap();
        assert_eq!(expected, actual);
    }
}
