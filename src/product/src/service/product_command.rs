use crate::core::description::Description;
use crate::core::product_image::ProductImage;
use crate::core::title::Title;
use common::currency::domain::Currency;
use common::error::missing_field::MissingRequiredField;
use common::has_key::HasKey;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{MonetaryAmount, Price};
use common::product_id::ProductKey;
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shops_product_id::ShopsProductId;
use field::field;
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
    pub state: ProductState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PipedProductCommand {
    pub shop_id: Option<ShopId>,
    pub shops_product_id: ShopsProductId,
    pub shop_name: Option<ShopName>,
    pub shop_type: Option<ShopType>,
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

#[cfg(feature = "data")]
impl From<crate::data::put_data::PutProductData> for PipedProductCommand {
    fn from(data: crate::data::put_data::PutProductData) -> Self {
        use crate::core::product_image::ProductImage;
        use crate::core::prohibited_content::ProhibitedContent;

        PipedProductCommand {
            shop_id: None,
            shops_product_id: data.shops_product_id,
            shop_name: None,
            shop_type: None,
            native_title: data.title.into(),
            other_title: Default::default(),
            native_description: data.description.map(Localized::from),
            other_description: Default::default(),
            native_price: data.price.map(Price::from),
            other_price: Default::default(),
            native_price_estimate_min: data.price_estimate_min.map(Price::from),
            other_price_estimate_min: Default::default(),
            native_price_estimate_max: data.price_estimate_max.map(Price::from),
            other_price_estimate_max: Default::default(),
            state: data.state.into(),
            url: data.url,
            images: data
                .images
                .into_iter()
                .map(|url| ProductImage {
                    url,
                    prohibited_content: ProhibitedContent::Unknown,
                })
                .collect(),
            auction_start: data.auction_start,
            auction_end: data.auction_end,
        }
    }
}

impl TryFrom<PipedProductCommand> for CreateProductCommand {
    type Error = MissingRequiredField;

    fn try_from(piped_cmd: PipedProductCommand) -> Result<Self, Self::Error> {
        let cmd = CreateProductCommand {
            shop_id: piped_cmd.shop_id.ok_or(MissingRequiredField::from(
                field!(shop_id@CreateProductCommand),
            ))?,
            shops_product_id: piped_cmd.shops_product_id,
            shop_name: piped_cmd.shop_name.ok_or(MissingRequiredField::from(
                field!(shop_name@CreateProductCommand),
            ))?,
            shop_type: piped_cmd.shop_type.ok_or(MissingRequiredField::from(
                field!(shop_type@CreateProductCommand),
            ))?,
            native_title: piped_cmd.native_title,
            other_title: piped_cmd.other_title,
            native_description: piped_cmd.native_description,
            other_description: piped_cmd.other_description,
            native_price: piped_cmd.native_price,
            other_price: piped_cmd.other_price,
            native_price_estimate_min: piped_cmd.native_price_estimate_min,
            other_price_estimate_min: piped_cmd.other_price_estimate_min,
            native_price_estimate_max: piped_cmd.native_price_estimate_max,
            other_price_estimate_max: piped_cmd.other_price_estimate_max,
            state: piped_cmd.state,
            url: piped_cmd.url,
            images: piped_cmd.images,
            auction_start: piped_cmd.auction_start,
            auction_end: piped_cmd.auction_end,
        };
        Ok(cmd)
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

    impl Dummy<Faker> for PipedProductCommand {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let native_price: Option<Price> = config.fake_with_rng(rng);
            let other_price = match native_price {
                None => HashMap::new(),
                Some(price) => FixedFxRate()
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap(),
            };
            let native_price_estimate_min: Option<Price> = config.fake_with_rng(rng);
            let other_price_estimate_min = match native_price_estimate_min {
                None => HashMap::new(),
                Some(price) => FixedFxRate()
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap(),
            };
            let native_price_estimate_max: Option<Price> = config.fake_with_rng(rng);
            let other_price_estimate_max = match native_price_estimate_max {
                None => HashMap::new(),
                Some(price) => FixedFxRate()
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap(),
            };
            let state = config.fake_with_rng(rng);
            PipedProductCommand {
                shop_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                shop_name: config.fake_with_rng(rng),
                shop_type: config.fake_with_rng(rng),
                native_title: config.fake_with_rng(rng),
                other_title: config.fake_with_rng(rng),
                native_description: config.fake_with_rng(rng),
                other_description: config.fake_with_rng(rng),
                native_price,
                other_price,
                native_price_estimate_min,
                other_price_estimate_min,
                native_price_estimate_max,
                other_price_estimate_max,
                state,
                url: Url::parse(&format!(
                    "https://foo.bar/item/{}",
                    config.fake_with_rng::<u16, _>(rng)
                ))
                .unwrap(),
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

    #[cfg(test)]
    mod tests {
        use crate::service::product_command::{
            CreateProductCommand, PipedProductCommand, UpdateProductCommand,
        };
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
        fn should_fake_piped_product_command() {
            let _ = Faker.fake::<PipedProductCommand>();
        }
    }
}
