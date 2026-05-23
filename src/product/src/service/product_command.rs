use crate::core::description::Description;
use crate::core::product_image::ProductImage;
use crate::core::title::Title;
use common::currency::domain::Currency;
use common::has_key::HasKey;
use common::language::domain::Language;
use common::localized::Localized;
use common::mergeable::Mergeable;
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
pub struct Translation<T> {
    pub source: Localized<Language, T>,
    pub targets: HashMap<Language, T>,
}

impl<T> Mergeable for Translation<T> {
    fn merge(&mut self, other: Self) {
        let Self { source, targets } = other;
        self.source = source;
        self.targets.extend(targets);
    }
}

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

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateProductCommand {
    pub native_price: Option<Price>,
    pub state: Option<ProductState>,
    pub native_price_estimate_min: Option<Price>,
    pub native_price_estimate_max: Option<Price>,
    pub url: Option<Url>,
    pub images: Option<Vec<ProductImage>>,
    pub auction_start: Option<OffsetDateTime>,
    pub auction_end: Option<OffsetDateTime>,
    pub embedding: Option<Vec<f32>>,
    pub translated_titles: Option<Translation<Title>>,
}

impl Mergeable for UpdateProductCommand {
    fn merge(&mut self, other: Self) {
        let Self {
            native_price,
            state,
            native_price_estimate_min,
            native_price_estimate_max,
            url,
            images,
            auction_start,
            auction_end,
            embedding,
            translated_titles,
        } = other;

        if let Some(native_price) = native_price {
            self.native_price = Some(native_price);
        }
        if let Some(state) = state {
            self.state = Some(state);
        }
        if let Some(native_price_estimate_min) = native_price_estimate_min {
            self.native_price_estimate_min = Some(native_price_estimate_min);
        }
        if let Some(native_price_estimate_max) = native_price_estimate_max {
            self.native_price_estimate_max = Some(native_price_estimate_max);
        }
        if let Some(url) = url {
            self.url = Some(url);
        }
        if let Some(images) = images {
            self.images = Some(images);
        }
        if let Some(auction_start) = auction_start {
            self.auction_start = Some(auction_start);
        }
        if let Some(auction_end) = auction_end {
            self.auction_end = Some(auction_end);
        }
        if let Some(embedding) = embedding {
            self.embedding = Some(embedding);
        }
        match (&mut self.translated_titles, translated_titles) {
            (Some(current), Some(other)) => current.merge(other),
            (None, Some(other)) => self.translated_titles = Some(other),
            _ => {}
        }
    }
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

impl Mergeable for UpsertProductCommand {
    fn merge(&mut self, other: Self) {
        let Self {
            shop_id,
            shops_product_id,
            seller_name_raw,
            structured_address,
            geo_address,
            native_title,
            native_description,
            native_price,
            native_price_estimate_min,
            native_price_estimate_max,
            state,
            url,
            images,
            auction_start,
            auction_end,
        } = other;

        debug_assert_eq!(self.shop_id, shop_id);
        debug_assert_eq!(self.shops_product_id, shops_product_id);

        if let Some(seller_name_raw) = seller_name_raw {
            self.seller_name_raw = Some(seller_name_raw);
        }
        if let Some(structured_address) = structured_address {
            self.structured_address = Some(structured_address);
        }
        if let Some(geo_address) = geo_address {
            self.geo_address = Some(geo_address);
        }
        if let Some(native_title) = native_title {
            self.native_title = Some(native_title);
        }
        if let Some(native_description) = native_description {
            self.native_description = Some(native_description);
        }
        if let Some(native_price) = native_price {
            self.native_price = Some(native_price);
        }
        if let Some(native_price_estimate_min) = native_price_estimate_min {
            self.native_price_estimate_min = Some(native_price_estimate_min);
        }
        if let Some(native_price_estimate_max) = native_price_estimate_max {
            self.native_price_estimate_max = Some(native_price_estimate_max);
        }
        if let Some(state) = state {
            self.state = Some(state);
        }
        if let Some(url) = url {
            self.url = Some(url);
        }
        self.images = images;
        if let Some(auction_start) = auction_start {
            self.auction_start = Some(auction_start);
        }
        if let Some(auction_end) = auction_end {
            self.auction_end = Some(auction_end);
        }
    }
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
            embedding: None,
            translated_titles: None,
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
                embedding: None,
                translated_titles: None,
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

#[cfg(test)]
mod tests {
    use super::{Translation, UpdateProductCommand, UpsertProductCommand};
    use crate::core::{product_image::ProductImage, title::Title};
    use common::{
        currency::domain::Currency, language::domain::Language, localized::Localized,
        mergeable::Mergeable, price::domain::Price, product_state::domain::ProductState,
    };
    use geo::core::address::{GeoAddress, StructuredAddress};
    use std::collections::HashMap;
    use url::Url;

    #[test]
    fn should_merge_update_product_command() {
        let mut current = UpdateProductCommand {
            native_price: Some(Price::new(100u64.into(), Currency::Eur)),
            state: None,
            native_price_estimate_min: None,
            native_price_estimate_max: None,
            url: None,
            images: None,
            auction_start: None,
            auction_end: None,
            embedding: None,
            translated_titles: Some(Translation {
                source: Localized::new(Language::De, Title::from("Stuhl")),
                targets: HashMap::from([(Language::En, Title::from("Chair"))]),
            }),
        };

        current.merge(UpdateProductCommand {
            native_price: None,
            state: Some(ProductState::Reserved),
            native_price_estimate_min: None,
            native_price_estimate_max: None,
            url: Some(Url::parse("https://example.com/product").unwrap()),
            images: Some(vec![ProductImage {
                url: Url::parse("https://example.com/product/image.jpg").unwrap(),
                prohibited_content: Default::default(),
            }]),
            auction_start: None,
            auction_end: None,
            embedding: Some(vec![0.1, 0.2, 0.3]),
            translated_titles: Some(Translation {
                source: Localized::new(Language::De, Title::from("Stuhl")),
                targets: HashMap::from([(Language::Fr, Title::from("Chaise"))]),
            }),
        });

        assert_eq!(
            Some(Price::new(100u64.into(), Currency::Eur)),
            current.native_price
        );
        assert_eq!(Some(ProductState::Reserved), current.state);
        assert_eq!(
            Some(Url::parse("https://example.com/product").unwrap()),
            current.url
        );
        assert_eq!(Some(vec![0.1, 0.2, 0.3]), current.embedding);
        let translated_titles = current
            .translated_titles
            .expect("translations should exist");
        assert_eq!(
            Some(&Title::from("Chair")),
            translated_titles.targets.get(&Language::En)
        );
        assert_eq!(
            Some(&Title::from("Chaise")),
            translated_titles.targets.get(&Language::Fr)
        );
    }

    #[test]
    fn should_merge_upsert_product_command() {
        let mut current = UpsertProductCommand {
            shop_id: Default::default(),
            shops_product_id: "shops-product-id".into(),
            seller_name_raw: None,
            structured_address: None,
            geo_address: None,
            native_title: Some(Localized::new(Language::En, Title::from("Chair"))),
            native_description: None,
            native_price: Some(Price::new(100u64.into(), Currency::Eur)),
            native_price_estimate_min: None,
            native_price_estimate_max: None,
            state: None,
            url: None,
            images: vec![],
            auction_start: None,
            auction_end: None,
        };

        current.merge(UpsertProductCommand {
            shop_id: current.shop_id,
            shops_product_id: current.shops_product_id.clone(),
            seller_name_raw: Some("seller".to_string()),
            structured_address: Some(StructuredAddress::default()),
            geo_address: Some(GeoAddress { lat: 1.0, lon: 2.0 }),
            native_title: None,
            native_description: None,
            native_price: None,
            native_price_estimate_min: None,
            native_price_estimate_max: None,
            state: Some(ProductState::Available),
            url: Some(Url::parse("https://example.com/product").unwrap()),
            images: vec![ProductImage {
                url: Url::parse("https://example.com/product/image.jpg").unwrap(),
                prohibited_content: Default::default(),
            }],
            auction_start: None,
            auction_end: None,
        });

        assert_eq!(Some("seller".to_string()), current.seller_name_raw);
        assert_eq!(Some(ProductState::Available), current.state);
        assert_eq!(
            Some(Url::parse("https://example.com/product").unwrap()),
            current.url
        );
        assert_eq!(1, current.images.len());
        assert_eq!(
            Some(Price::new(100u64.into(), Currency::Eur)),
            current.native_price
        );
        assert!(current.structured_address.is_some());
        assert!(current.geo_address.is_some());
    }
}
