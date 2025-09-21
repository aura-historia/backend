use common::has_key::HasKey;
use common::item_id::ItemKey;
use common::item_state::domain::ItemState;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::Price;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shops_item_id::ShopsItemId;
use item_core::description::Description;
use item_core::title::Title;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct PutItemCommand {
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
    pub shop_name: ShopName,
    pub title: Localized<Language, Title>,
    pub description: Option<Localized<Language, Description>>,
    pub price: Option<Price>,
    pub state: ItemState,
    pub url: Url,
    pub images: Vec<Url>,
}

impl HasKey for PutItemCommand {
    type Key = ItemKey;

    fn key(&self) -> Self::Key {
        ItemKey {
            shop_id: self.shop_id.clone(),
            shops_item_id: self.shops_item_id.clone(),
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for PutItemCommand {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            PutItemCommand {
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
        use crate::item_command::PutItemCommand;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_create_item_command() {
            let _ = Faker.fake::<PutItemCommand>();
        }
    }
}
