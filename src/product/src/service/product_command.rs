use crate::core::authenticity::Authenticity;
use crate::core::condition::Condition;
use crate::core::description::Description;
use crate::core::origin_year::OriginYear;
use crate::core::product_image::ProductImage;
use crate::core::provenance::Provenance;
use crate::core::restoration::Restoration;
use crate::core::title::Title;
use common::category_key::CategoryId;
use common::currency::domain::Currency;
use common::has_key::HasKey;
use common::language::domain::Language;
use common::localized::Localized;
use common::period_key::PeriodId;
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
    pub origin_year: Option<OriginYear>,
    pub authenticity: Authenticity,
    pub condition: Condition,
    pub provenance: Provenance,
    pub restoration: Restoration,
    pub category_id: Option<CategoryId>,
    pub period_id: Option<PeriodId>,
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

#[derive(Debug, Clone, PartialEq)]
pub struct UpsertProductCommand {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub shop_type: ShopType,
    pub native_title: Option<Localized<Language, Title>>,
    pub native_description: Option<Localized<Language, Description>>,
    pub native_price: Option<Price>,
    pub native_price_estimate_min: Option<Price>,
    pub native_price_estimate_max: Option<Price>,
    pub state: Option<ProductState>,
    pub url: Option<Url>,
    pub images: Vec<ProductImage>,
    pub auction_start: Option<OffsetDateTime>,
    pub auction_end: Option<OffsetDateTime>,
    pub origin_year: Option<OriginYear>,
    pub authenticity: Authenticity,
    pub condition: Condition,
    pub provenance: Provenance,
    pub restoration: Restoration,
}

impl HasKey for UpsertProductCommand {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        ProductKey {
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id.clone(),
        }
    }
}

impl UpsertProductCommand {
    pub fn to_create_command(self) -> CreateProductCommand {
        CreateProductCommand {
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id,
            shop_name: self.shop_name,
            shop_type: self.shop_type,
            native_title: self
                .native_title
                .unwrap_or_else(|| Localized::new(Language::En, Title::from(""))),
            other_title: HashMap::new(),
            native_description: self.native_description,
            other_description: HashMap::new(),
            native_price: self.native_price,
            other_price: HashMap::new(),
            native_price_estimate_min: self.native_price_estimate_min,
            other_price_estimate_min: HashMap::new(),
            native_price_estimate_max: self.native_price_estimate_max,
            other_price_estimate_max: HashMap::new(),
            state: self.state.unwrap_or(ProductState::Listed),
            url: self.url.unwrap_or_else(|| {
                Url::parse("https://not-provided.invalid").expect("static URL must be valid")
            }),
            images: self.images,
            auction_start: self.auction_start,
            auction_end: self.auction_end,
            origin_year: self.origin_year,
            authenticity: self.authenticity,
            condition: self.condition,
            provenance: self.provenance,
            restoration: self.restoration,
            category_id: None,
            period_id: None,
        }
    }

    pub fn to_update_command(&self) -> UpdateProductCommand {
        UpdateProductCommand {
            native_price: self.native_price,
            state: self.state,
        }
    }
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
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                category_id: None,
                period_id: None,
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

    impl Dummy<Faker> for UpsertProductCommand {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            UpsertProductCommand {
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                shop_name: config.fake_with_rng(rng),
                shop_type: config.fake_with_rng(rng),
                native_title: Some(config.fake_with_rng(rng)),
                native_description: config.fake_with_rng(rng),
                native_price: config.fake_with_rng(rng),
                native_price_estimate_min: config.fake_with_rng(rng),
                native_price_estimate_max: config.fake_with_rng(rng),
                state: config.fake_with_rng(rng),
                url: Some(Url::parse("https://www.example.com/product/1").unwrap()),
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
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::service::product_command::{
            CreateProductCommand, UpdateProductCommand, UpsertProductCommand,
        };
        use common::has_key::HasKey;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_create_product_command() {
            let _ = Faker.fake::<CreateProductCommand>();
        }

        #[test]
        fn should_fake_update_product_command() {
            let _ = Faker.fake::<UpdateProductCommand>();
        }

        #[test]
        fn should_fake_upsert_product_command() {
            let _ = Faker.fake::<UpsertProductCommand>();
        }

        #[test]
        fn should_convert_upsert_to_create_command() {
            let upsert: UpsertProductCommand = Faker.fake();
            let key = upsert.key();
            let create = upsert.to_create_command();
            assert_eq!(create.shop_id, key.shop_id);
            assert_eq!(create.shops_product_id, key.shops_product_id);
        }

        #[test]
        fn should_convert_upsert_to_update_command() {
            let upsert: UpsertProductCommand = Faker.fake();
            let update = upsert.to_update_command();
            assert_eq!(update.native_price, upsert.native_price);
            assert_eq!(update.state, upsert.state);
        }
    }
}
