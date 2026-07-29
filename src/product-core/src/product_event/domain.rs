use crate::description::Description;
use crate::product_image::ProductImage;
use crate::title::Title;
use common::currency::domain::Currency;
use common::has_key::HasKey;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{MonetaryAmount, Price};
use common::product_id::ProductKey;
use common::product_slug_id::ProductSlugId;
use common::product_state::domain::ProductState;
use common::seller_slug_id::SellerSlugId;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::shops_product_id::ShopsProductId;
use geo::core::address::{GeoAddress, StructuredAddress};
use indexmap::IndexSet;
use shop_core::shop_type::ShopType;
use std::collections::HashMap;
use time::OffsetDateTime;
use url::Url;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ProductDomainEventPayload {
    Created(ProductCreatedDomainEventPayload),
    StateChanged(ProductStateChangeDomainEventPayload),
    PriceChanged(ProductPriceChangeDomainEventPayload),
    EstimatePriceChanged(ProductEstimatePriceChangeDomainEventPayload),
    UrlChanged(ProductUrlChangeDomainEventPayload),
    ImagesChanged(ProductImagesChangeDomainEventPayload),
    AuctionTimeChanged(ProductAuctionTimeChangeDomainEventPayload),
}

impl ProductDomainEventPayload {
    pub fn event_type(&self) -> &'static str {
        match self {
            ProductDomainEventPayload::Created(_) => "DOMAIN_CREATED",
            ProductDomainEventPayload::StateChanged(_) => "DOMAIN_STATE_CHANGED",
            ProductDomainEventPayload::PriceChanged(_) => "DOMAIN_PRICE_CHANGED",
            ProductDomainEventPayload::EstimatePriceChanged(_) => "DOMAIN_ESTIMATE_PRICE_CHANGED",
            ProductDomainEventPayload::UrlChanged(_) => "DOMAIN_URL_CHANGED",
            ProductDomainEventPayload::ImagesChanged(_) => "DOMAIN_IMAGES_CHANGED",
            ProductDomainEventPayload::AuctionTimeChanged(_) => "DOMAIN_AUCTION_TIME_CHANGED",
        }
    }

    pub fn is_price_event(&self) -> bool {
        matches!(self, ProductDomainEventPayload::PriceChanged(_))
    }

    pub fn is_state_event(&self) -> bool {
        matches!(self, ProductDomainEventPayload::StateChanged(_))
    }

    pub fn as_created(&self) -> Option<&ProductCreatedDomainEventPayload> {
        match self {
            ProductDomainEventPayload::Created(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_state_changed(&self) -> Option<&ProductStateChangeDomainEventPayload> {
        match self {
            ProductDomainEventPayload::StateChanged(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_new_state(&self) -> Option<ProductState> {
        match self {
            ProductDomainEventPayload::StateChanged(payload) => Some(payload.new_state),
            _ => None,
        }
    }

    pub fn as_price_changed(&self) -> Option<&ProductPriceChangeDomainEventPayload> {
        match self {
            ProductDomainEventPayload::PriceChanged(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn localized(self, currency: &Currency) -> LocalizedProductDomainEventPayloadView {
        match self {
            ProductDomainEventPayload::Created(payload) => {
                let mut prices = payload.other_price;
                if let Some(native_price) = payload.native_price {
                    prices.insert(native_price.currency, native_price.monetary_amount);
                }
                LocalizedProductDomainEventPayloadView::Created(
                    LocalizedProductCreatedDomainEventPayloadView {
                        shop_id: payload.shop_id,
                        seller_id: payload.seller_id,
                        shops_product_id: payload.shops_product_id,
                        shop_name: payload.shop_name,
                        seller_name: payload.seller_name,
                        shop_type: payload.shop_type,
                        title: payload.native_title,
                        description: payload.native_description,
                        price: prices
                            .remove(currency)
                            .map(|amount| Price::new(amount, *currency)),
                        state: payload.state,
                        url: payload.url,
                        images: payload.images,
                    },
                )
            }
            ProductDomainEventPayload::StateChanged(payload) => {
                LocalizedProductDomainEventPayloadView::StateChanged(
                    LocalizedProductStateChangeDomainEventPayloadView {
                        shop_id: payload.shop_id,
                        seller_id: payload.seller_id,
                        shops_product_id: payload.shops_product_id,
                        old_state: payload.old_state,
                        new_state: payload.new_state,
                    },
                )
            }
            ProductDomainEventPayload::PriceChanged(payload) => {
                let old_price = match payload.old_native_price {
                    Some(old_native_price) => {
                        let mut old_prices = payload.old_other_price;
                        old_prices
                            .insert(old_native_price.currency, old_native_price.monetary_amount);
                        Currency::resolve(&[*currency], old_prices)
                    }
                    None => None,
                };
                let new_price = match payload.new_native_price {
                    Some(new_native_price) => {
                        let mut new_prices = payload.new_other_price;
                        new_prices
                            .insert(new_native_price.currency, new_native_price.monetary_amount);
                        Currency::resolve(&[*currency], new_prices)
                    }
                    None => None,
                };
                LocalizedProductDomainEventPayloadView::PriceChanged(
                    LocalizedProductPriceChangeDomainEventPayloadView {
                        shop_id: payload.shop_id,
                        seller_id: payload.seller_id,
                        shops_product_id: payload.shops_product_id,
                        old_price,
                        new_price,
                    },
                )
            }
            ProductDomainEventPayload::EstimatePriceChanged(payload) => {
                let price_estimate_min = match payload.native_price_estimate_min {
                    Some(native_min) => {
                        let mut min_prices = payload.other_price_estimate_min;
                        min_prices.insert(native_min.currency, native_min.monetary_amount);
                        Currency::resolve(&[*currency], min_prices)
                    }
                    None => None,
                };
                let price_estimate_max = match payload.native_price_estimate_max {
                    Some(native_max) => {
                        let mut max_prices = payload.other_price_estimate_max;
                        max_prices.insert(native_max.currency, native_max.monetary_amount);
                        Currency::resolve(&[*currency], max_prices)
                    }
                    None => None,
                };
                LocalizedProductDomainEventPayloadView::EstimatePriceChanged(
                    LocalizedProductEstimatePriceChangeDomainEventPayloadView {
                        shop_id: payload.shop_id,
                        seller_id: payload.seller_id,
                        shops_product_id: payload.shops_product_id,
                        price_estimate_min,
                        price_estimate_max,
                    },
                )
            }
            ProductDomainEventPayload::UrlChanged(payload) => {
                LocalizedProductDomainEventPayloadView::UrlChanged(
                    LocalizedProductUrlChangeDomainEventPayloadView {
                        shop_id: payload.shop_id,
                        seller_id: payload.seller_id,
                        shops_product_id: payload.shops_product_id,
                        url: payload.url,
                    },
                )
            }
            ProductDomainEventPayload::ImagesChanged(payload) => {
                LocalizedProductDomainEventPayloadView::ImagesChanged(
                    LocalizedProductImagesChangeDomainEventPayloadView {
                        shop_id: payload.shop_id,
                        seller_id: payload.seller_id,
                        shops_product_id: payload.shops_product_id,
                        images: payload.images,
                    },
                )
            }
            ProductDomainEventPayload::AuctionTimeChanged(payload) => {
                LocalizedProductDomainEventPayloadView::AuctionTimeChanged(
                    LocalizedProductAuctionTimeChangeDomainEventPayloadView {
                        shop_id: payload.shop_id,
                        seller_id: payload.seller_id,
                        shops_product_id: payload.shops_product_id,
                        auction_start: payload.auction_start,
                        auction_end: payload.auction_end,
                    },
                )
            }
        }
    }
}

impl HasKey for ProductDomainEventPayload {
    type Key = ProductKey;

    fn key(&self) -> ProductKey {
        ProductKey::new(*self.shop_id(), self.shops_product_id().clone())
    }
}

pub trait ProductCommonEventPayload {
    fn shop_id(&self) -> &ShopId;
    fn shops_product_id(&self) -> &ShopsProductId;
    fn seller_id(&self) -> &ShopId;
}

impl ProductCommonEventPayload for ProductDomainEventPayload {
    fn shop_id(&self) -> &ShopId {
        match self {
            ProductDomainEventPayload::Created(payload) => payload.shop_id(),
            ProductDomainEventPayload::StateChanged(payload) => payload.shop_id(),
            ProductDomainEventPayload::PriceChanged(payload) => payload.shop_id(),
            ProductDomainEventPayload::EstimatePriceChanged(payload) => payload.shop_id(),
            ProductDomainEventPayload::UrlChanged(payload) => payload.shop_id(),
            ProductDomainEventPayload::ImagesChanged(payload) => payload.shop_id(),
            ProductDomainEventPayload::AuctionTimeChanged(payload) => payload.shop_id(),
        }
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        match self {
            ProductDomainEventPayload::Created(payload) => payload.shops_product_id(),
            ProductDomainEventPayload::StateChanged(payload) => payload.shops_product_id(),
            ProductDomainEventPayload::PriceChanged(payload) => payload.shops_product_id(),
            ProductDomainEventPayload::EstimatePriceChanged(payload) => payload.shops_product_id(),
            ProductDomainEventPayload::UrlChanged(payload) => payload.shops_product_id(),
            ProductDomainEventPayload::ImagesChanged(payload) => payload.shops_product_id(),
            ProductDomainEventPayload::AuctionTimeChanged(payload) => payload.shops_product_id(),
        }
    }

    fn seller_id(&self) -> &ShopId {
        match self {
            ProductDomainEventPayload::Created(payload) => payload.seller_id(),
            ProductDomainEventPayload::StateChanged(payload) => payload.seller_id(),
            ProductDomainEventPayload::PriceChanged(payload) => payload.seller_id(),
            ProductDomainEventPayload::EstimatePriceChanged(payload) => payload.seller_id(),
            ProductDomainEventPayload::UrlChanged(payload) => payload.seller_id(),
            ProductDomainEventPayload::ImagesChanged(payload) => payload.seller_id(),
            ProductDomainEventPayload::AuctionTimeChanged(payload) => payload.seller_id(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductCreatedDomainEventPayload {
    pub product_slug_id: ProductSlugId,
    pub shop_slug_id: ShopSlugId,
    pub seller_slug_id: SellerSlugId,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub seller_name: ShopName,
    pub shop_type: ShopType,
    pub structured_address: Option<StructuredAddress>,
    pub geo_address: Option<GeoAddress>,
    pub native_title: Localized<Language, Title>,
    pub native_description: Option<Localized<Language, Description>>,
    pub native_price: Option<Price>,
    pub other_price: HashMap<Currency, MonetaryAmount>,
    pub native_price_estimate_min: Option<Price>,
    pub other_price_estimate_min: HashMap<Currency, MonetaryAmount>,
    pub native_price_estimate_max: Option<Price>,
    pub other_price_estimate_max: HashMap<Currency, MonetaryAmount>,
    pub state: ProductState,
    pub url: Url,
    pub view_url: Url,
    pub images: IndexSet<ProductImage>,
    pub auction_start: Option<OffsetDateTime>,
    pub auction_end: Option<OffsetDateTime>,
}

impl ProductCommonEventPayload for ProductCreatedDomainEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        &self.shops_product_id
    }

    fn seller_id(&self) -> &ShopId {
        &self.seller_id
    }
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
pub struct ProductStateChangeDomainEventPayload {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub old_state: ProductState,
    pub new_state: ProductState,
}

impl ProductCommonEventPayload for ProductStateChangeDomainEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        &self.shops_product_id
    }

    fn seller_id(&self) -> &ShopId {
        &self.seller_id
    }
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
pub struct ProductPriceChangeDomainEventPayload {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub old_native_price: Option<Price>,
    pub old_other_price: HashMap<Currency, MonetaryAmount>,
    pub new_native_price: Option<Price>,
    pub new_other_price: HashMap<Currency, MonetaryAmount>,
}

impl ProductCommonEventPayload for ProductPriceChangeDomainEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        &self.shops_product_id
    }

    fn seller_id(&self) -> &ShopId {
        &self.seller_id
    }
}

impl ProductPriceChangeDomainEventPayload {
    pub fn old_prices(&self) -> HashMap<Currency, MonetaryAmount> {
        let mut prices = self.old_other_price.clone();
        if let Some(price) = self.old_native_price {
            prices.insert(price.currency, price.monetary_amount);
        }
        prices
    }

    pub fn new_prices(&self) -> HashMap<Currency, MonetaryAmount> {
        let mut prices = self.new_other_price.clone();
        if let Some(price) = self.new_native_price {
            prices.insert(price.currency, price.monetary_amount);
        }
        prices
    }
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
pub struct ProductEstimatePriceChangeDomainEventPayload {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub native_price_estimate_min: Option<Price>,
    pub other_price_estimate_min: HashMap<Currency, MonetaryAmount>,
    pub native_price_estimate_max: Option<Price>,
    pub other_price_estimate_max: HashMap<Currency, MonetaryAmount>,
}

impl ProductCommonEventPayload for ProductEstimatePriceChangeDomainEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        &self.shops_product_id
    }

    fn seller_id(&self) -> &ShopId {
        &self.seller_id
    }
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
pub struct ProductUrlChangeDomainEventPayload {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub url: Url,
    pub view_url: Url,
}

impl ProductCommonEventPayload for ProductUrlChangeDomainEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        &self.shops_product_id
    }

    fn seller_id(&self) -> &ShopId {
        &self.seller_id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductImagesChangeDomainEventPayload {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub images: IndexSet<ProductImage>,
}

impl ProductCommonEventPayload for ProductImagesChangeDomainEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        &self.shops_product_id
    }

    fn seller_id(&self) -> &ShopId {
        &self.seller_id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductAuctionTimeChangeDomainEventPayload {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub auction_start: Option<OffsetDateTime>,
    pub auction_end: Option<OffsetDateTime>,
}

impl ProductCommonEventPayload for ProductAuctionTimeChangeDomainEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_product_id(&self) -> &ShopsProductId {
        &self.shops_product_id
    }

    fn seller_id(&self) -> &ShopId {
        &self.seller_id
    }
}

#[cfg(feature = "test-data")]
mod faker_payloads {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ProductCreatedDomainEventPayload {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductCreatedDomainEventPayload {
                product_slug_id: config.fake_with_rng(rng),
                shop_slug_id: config.fake_with_rng(rng),
                seller_slug_id: config.fake_with_rng(rng),
                shop_id: config.fake_with_rng(rng),
                seller_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                shop_name: config.fake_with_rng(rng),
                seller_name: config.fake_with_rng(rng),
                shop_type: config.fake_with_rng(rng),
                structured_address: config.fake_with_rng(rng),
                geo_address: config.fake_with_rng(rng),
                native_title: config.fake_with_rng(rng),
                native_description: config.fake_with_rng(rng),
                native_price: config.fake_with_rng(rng),
                other_price: config.fake_with_rng(rng),
                native_price_estimate_min: config.fake_with_rng(rng),
                other_price_estimate_min: config.fake_with_rng(rng),
                native_price_estimate_max: config.fake_with_rng(rng),
                other_price_estimate_max: config.fake_with_rng(rng),
                state: config.fake_with_rng(rng),
                url: Url::parse("https://www.example.com/product").unwrap(),
                view_url: Url::parse("https://www.example.com/product?utm_source=aura-historia")
                    .unwrap(),
                images: config
                    .fake_with_rng::<Vec<ProductImage>, _>(rng)
                    .into_iter()
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

    impl Dummy<Faker> for ProductAuctionTimeChangeDomainEventPayload {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductAuctionTimeChangeDomainEventPayload {
                shop_id: config.fake_with_rng(rng),
                seller_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
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

    impl Dummy<Faker> for ProductImagesChangeDomainEventPayload {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductImagesChangeDomainEventPayload {
                shop_id: config.fake_with_rng(rng),
                seller_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                images: config
                    .fake_with_rng::<Vec<ProductImage>, _>(rng)
                    .into_iter()
                    .collect(),
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum LocalizedProductDomainEventPayloadView {
    Created(LocalizedProductCreatedDomainEventPayloadView),
    StateChanged(LocalizedProductStateChangeDomainEventPayloadView),
    PriceChanged(LocalizedProductPriceChangeDomainEventPayloadView),
    EstimatePriceChanged(LocalizedProductEstimatePriceChangeDomainEventPayloadView),
    UrlChanged(LocalizedProductUrlChangeDomainEventPayloadView),
    ImagesChanged(LocalizedProductImagesChangeDomainEventPayloadView),
    AuctionTimeChanged(LocalizedProductAuctionTimeChangeDomainEventPayloadView),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedProductCreatedDomainEventPayloadView {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub seller_name: ShopName,
    pub shop_type: ShopType,
    pub title: Localized<Language, Title>,
    pub description: Option<Localized<Language, Description>>,
    pub price: Option<Price>,
    pub state: ProductState,
    pub url: Url,
    pub images: IndexSet<ProductImage>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedProductStateChangeDomainEventPayloadView {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub old_state: ProductState,
    pub new_state: ProductState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedProductPriceChangeDomainEventPayloadView {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub old_price: Option<Price>,
    pub new_price: Option<Price>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedProductEstimatePriceChangeDomainEventPayloadView {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub price_estimate_min: Option<Price>,
    pub price_estimate_max: Option<Price>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedProductUrlChangeDomainEventPayloadView {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub url: Url,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedProductImagesChangeDomainEventPayloadView {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub images: IndexSet<ProductImage>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedProductAuctionTimeChangeDomainEventPayloadView {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub auction_start: Option<OffsetDateTime>,
    pub auction_end: Option<OffsetDateTime>,
}
