use crate::core::description::Description;
use crate::core::product_image::ProductImage;
use crate::core::title::Title;
use common::currency::domain::Currency;
use common::has_key::HasKey;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{MonetaryAmount, Price};
use common::product_id::ProductKey;
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shops_product_id::ShopsProductId;
use shop::core::shop_type::ShopType;
use std::collections::HashMap;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateProductCommand {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub shop_type: ShopType,
    pub native_title: Localized<Language, Title>,
    pub other_title: HashMap<Language, Title>,
    pub native_description: Option<Localized<Language, Description>>,
    pub other_description: HashMap<Language, Description>,
    pub native_price: Option<Price>,
    pub other_price: HashMap<Currency, MonetaryAmount>,
    pub native_price_estimate_min: Option<Price>,
    pub other_price_estimate_min: HashMap<Currency, MonetaryAmount>,
    pub native_price_estimate_max: Option<Price>,
    pub other_price_estimate_max: HashMap<Currency, MonetaryAmount>,
    pub state: ProductState,
    pub url: Url,
    pub images: Vec<ProductImage>,
    pub auction_start: Option<OffsetDateTime>,
    pub auction_end: Option<OffsetDateTime>,
}

impl HasKey for CreateProductCommand {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        ProductKey {
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateProductCommand {
    pub native_price: Option<Price>,
    pub state: Option<ProductState>,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use common::price::domain::{FixedFxRate, FxRate};
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for CreateProductCommand {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let native_price = config.fake_with_rng::<Option<Price>, R>(rng);
            let other_price = native_price.map(|price| {
                FixedFxRate()
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap()
            });

            let native_price_estimate_min = config.fake_with_rng::<Option<Price>, R>(rng);
            let other_price_estimate_min = native_price_estimate_min.map(|price| {
                FixedFxRate()
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap()
            });

            let native_price_estimate_max = config.fake_with_rng::<Option<Price>, R>(rng);
            let other_price_estimate_max = native_price_estimate_max.map(|price| {
                FixedFxRate()
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap()
            });

            CreateProductCommand {
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                shop_name: config.fake_with_rng(rng),
                shop_type: config.fake_with_rng(rng),
                native_title: config.fake_with_rng(rng),
                other_title: config.fake_with_rng(rng),
                native_description: config.fake_with_rng(rng),
                other_description: config.fake_with_rng(rng),
                native_price,
                other_price: other_price.unwrap_or_default(),
                native_price_estimate_min,
                other_price_estimate_min: other_price_estimate_min.unwrap_or_default(),
                native_price_estimate_max,
                other_price_estimate_max: other_price_estimate_max.unwrap_or_default(),
                state: config.fake_with_rng(rng),
                url: Url::parse("https://www.youtube.com/watch?v=dQw4w9WgXcQ").unwrap(),
                images: Faker.fake(),
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

    impl Dummy<Faker> for UpdateProductCommand {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            UpdateProductCommand {
                native_price: config.fake_with_rng(rng),
                state: config.fake_with_rng(rng),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::service::product_command::{CreateProductCommand, UpdateProductCommand};
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_create_product_command() {
            let _ = Faker.fake::<CreateProductCommand>();
        }

        #[test]
        fn should_fake_update_product_command() {
            let _ = Faker.fake::<UpdateProductCommand>();
        }
    }
}
