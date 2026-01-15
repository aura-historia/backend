use crate::data::product_state_data::ProductStateData;
use common::language::data::LocalizedTextData;
use common::price::data::PriceData;
use common::shops_product_id::ShopsProductId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PutProductData {
    pub shops_product_id: ShopsProductId,

    pub title: LocalizedTextData,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<LocalizedTextData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min: Option<PriceData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max: Option<PriceData>,

    pub state: ProductStateData,

    pub url: Url,

    #[serde(default)]
    pub images: Vec<Url>,

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
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use crate::core::{description::Description, title::Title};
    use common::{fake::url::ImageUrl, language::data::LanguageData};
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for PutProductData {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let image_count = rng.random_range(0..=7);
            PutProductData {
                shops_product_id: config.fake_with_rng(rng),
                title: LocalizedTextData {
                    text: config.fake_with_rng::<Title, R>(rng).into(),
                    language: if config.fake_with_rng(rng) {
                        LanguageData::De
                    } else {
                        config.fake_with_rng(rng)
                    },
                },
                description: if config.fake_with_rng(rng) {
                    Some(LocalizedTextData {
                        text: config.fake_with_rng::<Description, R>(rng).into(),
                        language: if config.fake_with_rng(rng) {
                            LanguageData::De
                        } else {
                            config.fake_with_rng(rng)
                        },
                    })
                } else {
                    None
                },
                price: config.fake_with_rng(rng),
                price_estimate_min: config.fake_with_rng(rng),
                price_estimate_max: config.fake_with_rng(rng),
                state: config.fake_with_rng(rng),
                url: config
                    .fake_with_rng::<Url, R>(rng)
                    .join(&config.fake_with_rng::<String, R>(rng))
                    .unwrap(),
                images: fake::vec![ImageUrl; image_count]
                    .into_iter()
                    .map(Url::from)
                    .collect(),
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
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::data::put_data::PutProductData;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_put_product_data() {
            let _ = Faker.fake::<PutProductData>();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::data::{product_state_data::ProductStateData, put_data::PutProductData};
    use common::{
        currency::data::CurrencyData,
        language::data::{LanguageData, LocalizedTextData},
        price::data::PriceData,
        shops_product_id::ShopsProductId,
    };
    use serde_json::json;
    use url::Url;

    #[test]
    fn should_deserialize_put_product_data() {
        let shops_product_id = ShopsProductId::new();
        let json = json!({
            "shopsProductId": shops_product_id,
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
        });

        let expected = PutProductData {
            shops_product_id: shops_product_id.clone(),
            title: LocalizedTextData::new("Mein titel", LanguageData::De),
            description: Some(LocalizedTextData::new("My description", LanguageData::En)),
            price: Some(PriceData::new(CurrencyData::Eur, 50000)),
            state: ProductStateData::Reserved,
            url: Url::parse("https://my-shop.de/item").unwrap(),
            images: vec![
                Url::parse("https://my-shop.de/item/images/1").unwrap(),
                Url::parse("https://my-shop.de/item/images/2").unwrap(),
            ],
            auction_start: None,
            auction_end: None,
        };

        let actual = serde_json::from_value(json).unwrap();
        assert_eq!(expected, actual);
    }
}
