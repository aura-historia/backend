use std::collections::HashMap;

use common::currency::domain::Currency;
use common::error::missing_field::MissingRequiredField;
use common::has_key::HasKey;
use common::item_id::ItemKey;
use common::item_state::domain::ItemState;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{MonetaryAmount, Price};
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shops_item_id::ShopsItemId;
use field::field;
use item_core::description::Description;
use item_core::title::Title;
use url::Url;

use crate::enrichment_service::PipedItemCommand;

#[derive(Debug, Clone, PartialEq)]
pub struct UpsertItemCommand {
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
    pub shop_name: ShopName,
    pub native_title: Localized<Language, Title>,
    pub other_title: HashMap<Language, Title>,
    pub native_description: Option<Localized<Language, Description>>,
    pub other_description: HashMap<Language, Description>,
    pub native_price: Option<Price>,
    pub other_price: HashMap<Currency, MonetaryAmount>,
    pub state: ItemState,
    pub url: Url,
    pub images: Vec<Url>,
}

impl HasKey for UpsertItemCommand {
    type Key = ItemKey;

    fn key(&self) -> Self::Key {
        ItemKey {
            shop_id: self.shop_id,
            shops_item_id: self.shops_item_id.clone(),
        }
    }
}

impl TryFrom<PipedItemCommand> for UpsertItemCommand {
    type Error = MissingRequiredField;

    fn try_from(piped_cmd: PipedItemCommand) -> Result<Self, Self::Error> {
        let cmd = UpsertItemCommand {
            shop_id: piped_cmd.shop_id.ok_or(MissingRequiredField::from(
                field!(shop_id@UpsertItemCommand),
            ))?,
            shops_item_id: piped_cmd.shops_item_id,
            shop_name: piped_cmd.shop_name.ok_or(MissingRequiredField::from(
                field!(shop_name@UpsertItemCommand),
            ))?,
            native_title: piped_cmd.native_title,
            other_title: piped_cmd.other_title,
            native_description: piped_cmd.native_description,
            other_description: piped_cmd.other_description,
            native_price: piped_cmd.native_price,
            other_price: piped_cmd.other_price,
            state: piped_cmd.state,
            url: piped_cmd.url,
            images: piped_cmd.images,
        };
        Ok(cmd)
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use common::price::domain::{FixedFxRate, FxRate};
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for UpsertItemCommand {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let native_price = config.fake_with_rng::<Option<Price>, R>(rng);
            let other_price = native_price.map(|price| {
                FixedFxRate()
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap()
            });

            UpsertItemCommand {
                shop_id: config.fake_with_rng(rng),
                shops_item_id: config.fake_with_rng(rng),
                shop_name: config.fake_with_rng(rng),
                native_title: config.fake_with_rng(rng),
                other_title: config.fake_with_rng(rng),
                native_description: config.fake_with_rng(rng),
                other_description: config.fake_with_rng(rng),
                native_price,
                other_price: other_price.unwrap_or_default(),
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
        use crate::item_command::UpsertItemCommand;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_create_item_command() {
            let _ = Faker.fake::<UpsertItemCommand>();
        }
    }
}
