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
use common::shops_product_id::ShopsProductId;
use geo::core::address::{GeoAddress, StructuredAddress};
use std::collections::HashMap;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateProductCommand {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub seller_name_raw: Option<String>,
    pub structured_address: Option<StructuredAddress>,
    pub geo_address: Option<GeoAddress>,
    pub native_title: Localized<Language, Title>,
    pub other_title: HashMap<Language, Title>,
    pub native_description: Option<Localized<Language, Description>>,
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
    pub native_price_estimate_min: Option<Price>,
    pub native_price_estimate_max: Option<Price>,
    pub url: Option<Url>,
    pub images: Option<Vec<ProductImage>>,
    pub auction_start: Option<OffsetDateTime>,
    pub auction_end: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpsertProductCommand {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub seller_name_raw: Option<String>,
    pub structured_address: Option<StructuredAddress>,
    pub geo_address: Option<GeoAddress>,
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

impl From<UpsertProductCommand> for CreateProductCommand {
    fn from(cmd: UpsertProductCommand) -> Self {
        CreateProductCommand {
            shop_id: cmd.shop_id,
            shops_product_id: cmd.shops_product_id,
            seller_name_raw: cmd.seller_name_raw,
            structured_address: cmd.structured_address,
            geo_address: cmd.geo_address,
            native_title: cmd
                .native_title
                .unwrap_or_else(|| Localized::new(Language::En, Title::from(""))),
            other_title: HashMap::new(),
            native_description: cmd.native_description,
            native_price: cmd.native_price,
            other_price: HashMap::new(),
            native_price_estimate_min: cmd.native_price_estimate_min,
            other_price_estimate_min: HashMap::new(),
            native_price_estimate_max: cmd.native_price_estimate_max,
            other_price_estimate_max: HashMap::new(),
            state: cmd.state.unwrap_or(ProductState::Listed),
            url: cmd.url.unwrap_or_else(|| {
                Url::parse("https://not-provided.invalid").expect("static URL must be valid")
            }),
            images: cmd.images,
            auction_start: cmd.auction_start,
            auction_end: cmd.auction_end,
        }
    }
}

impl From<&UpsertProductCommand> for UpdateProductCommand {
    fn from(cmd: &UpsertProductCommand) -> Self {
        UpdateProductCommand {
            native_price: cmd.native_price,
            state: cmd.state,
            native_price_estimate_min: cmd.native_price_estimate_min,
            native_price_estimate_max: cmd.native_price_estimate_max,
            url: cmd.url.clone(),
            images: Some(cmd.images.clone()),
            auction_start: cmd.auction_start,
            auction_end: cmd.auction_end,
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
                seller_name_raw: config.fake_with_rng(rng),
                structured_address: config.fake_with_rng(rng),
                geo_address: config.fake_with_rng(rng),
                native_title: config.fake_with_rng(rng),
                other_title: config.fake_with_rng(rng),
                native_description: config.fake_with_rng(rng),
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
                native_price_estimate_min: config.fake_with_rng(rng),
                native_price_estimate_max: config.fake_with_rng(rng),
                url: Some(Url::parse("https://www.example.com/product/updated").unwrap()),
                images: Some(Faker.fake()),
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

    impl Dummy<Faker> for UpsertProductCommand {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            UpsertProductCommand {
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                seller_name_raw: config.fake_with_rng(rng),
                structured_address: config.fake_with_rng(rng),
                geo_address: config.fake_with_rng(rng),
                native_title: config.fake_with_rng(rng),
                native_description: config.fake_with_rng(rng),
                native_price: config.fake_with_rng(rng),
                native_price_estimate_min: config.fake_with_rng(rng),
                native_price_estimate_max: config.fake_with_rng(rng),
                state: config.fake_with_rng(rng),
                url: Some(Url::parse("https://www.example.com/product/upserted").unwrap()),
                images: config.fake_with_rng(rng),
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
}
