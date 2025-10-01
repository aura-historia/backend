use crate::item_state_data::ItemStateData;
use common::has_key::HasKey;
use common::item_id::ItemKey;
use common::language::data::LocalizedTextData;
use common::price::data::PriceData;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PutItemData {
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

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub images: Vec<Url>,
}

impl HasKey for PutItemData {
    type Key = ItemKey;

    fn key(&self) -> Self::Key {
        ItemKey {
            shop_id: self.shop_id,
            shops_item_id: self.shops_item_id.clone(),
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for PutItemData {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            PutItemData {
                shop_id: config.fake_with_rng(rng),
                shops_item_id: config.fake_with_rng(rng),
                shop_name: config.fake_with_rng(rng),
                title: config.fake_with_rng(rng),
                description: config.fake_with_rng(rng),
                price: config.fake_with_rng(rng),
                state: config.fake_with_rng(rng),
                url: Url::parse("https://www.youtube.com/watch?v=dQw4w9WgXcQ").unwrap(),
                images: vec![
                    Url::parse("https://fastly.picsum.photos/id/866/200/300.jpg?hmac=rcadCENKh4rD6MAp6V_ma-AyWv641M4iiOpe1RyFHeI").unwrap(),
                    Url::parse("https://fastly.picsum.photos/id/729/1080/720.jpg?hmac=87UNPD0SCY0yxDtSQzOiPil2OHh96KWCVg1qkqLuEns").unwrap(),
                    Url::parse("https://fastly.picsum.photos/id/729/1080/720.jpg?hmac=87UNPD0SCY0yxDtSQzOiPil2OHh96KWCVg1qkqLuEns").unwrap(),
                    Url::parse("https://fastly.picsum.photos/id/1082/1920/1080.jpg?hmac=R-FW85Ql3APTWaXe09q_4kjyylVzjB_EySE3UwZOrLU").unwrap(),
                    Url::parse("https://fachschaft.matheinfo.uni-halle.de/im/1270987911_1_0.jpg").unwrap(),
                ],
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::put_data::PutItemData;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_put_item_data() {
            let _ = Faker.fake::<PutItemData>();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{item_state_data::ItemStateData, put_data::PutItemData};
    use common::{
        currency::data::CurrencyData,
        language::data::{LanguageData, LocalizedTextData},
        price::data::PriceData,
        shop_id::ShopId,
        shops_item_id::ShopsItemId,
    };
    use serde_json::json;
    use url::Url;

    #[test]
    fn should_deserialize_put_item_data() {
        let shop_id = ShopId::new();
        let shops_item_id = ShopsItemId::new();
        let json = json!({
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
        });

        let expected = PutItemData {
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
        };

        let actual = serde_json::from_value(json).unwrap();
        assert_eq!(expected, actual);
    }
}
