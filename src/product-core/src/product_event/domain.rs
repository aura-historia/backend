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

#[cfg(test)]
mod tests {
    use super::*;

    fn price(amount: u64, currency: Currency) -> Price {
        Price::new(MonetaryAmount::from(amount), currency)
    }

    fn url(path: &str) -> Url {
        Url::parse(&format!("https://shop.example/{path}"))
            .unwrap_or_else(|error| panic!("invalid test URL: {error}"))
    }

    fn ids() -> (ShopId, ShopId, ShopsProductId) {
        (ShopId::new(), ShopId::new(), ShopsProductId::from("sku-1"))
    }

    fn created_payload() -> ProductCreatedDomainEventPayload {
        let (shop_id, seller_id, shops_product_id) = ids();
        ProductCreatedDomainEventPayload {
            product_slug_id: ProductSlugId::from("bronze-vase"),
            shop_slug_id: ShopSlugId::from("shop"),
            seller_slug_id: SellerSlugId::from("seller"),
            shop_id,
            seller_id,
            shops_product_id,
            shop_name: ShopName::from("Shop"),
            seller_name: ShopName::from("Seller"),
            shop_type: ShopType::CommercialDealer,
            structured_address: None,
            geo_address: None,
            native_title: Localized::new(Language::De, Title::from("Bronze Vase")),
            native_description: Some(Localized::new(Language::De, Description::from("Alt"))),
            native_price: Some(price(100, Currency::Eur)),
            other_price: [(Currency::Usd, MonetaryAmount::from(110_u64))].into(),
            native_price_estimate_min: Some(price(90, Currency::Eur)),
            other_price_estimate_min: [(Currency::Usd, MonetaryAmount::from(99_u64))].into(),
            native_price_estimate_max: Some(price(120, Currency::Eur)),
            other_price_estimate_max: [(Currency::Usd, MonetaryAmount::from(132_u64))].into(),
            state: ProductState::Listed,
            url: url("product"),
            view_url: url("product?utm_source=aura_historia"),
            images: IndexSet::new(),
            auction_start: Some(OffsetDateTime::UNIX_EPOCH),
            auction_end: None,
        }
    }

    fn state_payload() -> ProductStateChangeDomainEventPayload {
        let (shop_id, seller_id, shops_product_id) = ids();
        ProductStateChangeDomainEventPayload {
            shop_id,
            seller_id,
            shops_product_id,
            old_state: ProductState::Listed,
            new_state: ProductState::Available,
        }
    }

    fn price_payload() -> ProductPriceChangeDomainEventPayload {
        let (shop_id, seller_id, shops_product_id) = ids();
        ProductPriceChangeDomainEventPayload {
            shop_id,
            seller_id,
            shops_product_id,
            old_native_price: Some(price(100, Currency::Eur)),
            old_other_price: [(Currency::Usd, MonetaryAmount::from(110_u64))].into(),
            new_native_price: Some(price(200, Currency::Eur)),
            new_other_price: [(Currency::Usd, MonetaryAmount::from(220_u64))].into(),
        }
    }

    fn estimate_payload() -> ProductEstimatePriceChangeDomainEventPayload {
        let (shop_id, seller_id, shops_product_id) = ids();
        ProductEstimatePriceChangeDomainEventPayload {
            shop_id,
            seller_id,
            shops_product_id,
            native_price_estimate_min: Some(price(90, Currency::Eur)),
            other_price_estimate_min: [(Currency::Usd, MonetaryAmount::from(99_u64))].into(),
            native_price_estimate_max: Some(price(120, Currency::Eur)),
            other_price_estimate_max: [(Currency::Usd, MonetaryAmount::from(132_u64))].into(),
        }
    }

    fn url_payload() -> ProductUrlChangeDomainEventPayload {
        let (shop_id, seller_id, shops_product_id) = ids();
        ProductUrlChangeDomainEventPayload {
            shop_id,
            seller_id,
            shops_product_id,
            url: url("product"),
            view_url: url("product?utm_source=aura_historia"),
        }
    }

    fn images_payload() -> ProductImagesChangeDomainEventPayload {
        let (shop_id, seller_id, shops_product_id) = ids();
        ProductImagesChangeDomainEventPayload {
            shop_id,
            seller_id,
            shops_product_id,
            images: IndexSet::new(),
        }
    }

    fn auction_payload() -> ProductAuctionTimeChangeDomainEventPayload {
        let (shop_id, seller_id, shops_product_id) = ids();
        ProductAuctionTimeChangeDomainEventPayload {
            shop_id,
            seller_id,
            shops_product_id,
            auction_start: Some(OffsetDateTime::UNIX_EPOCH),
            auction_end: None,
        }
    }

    #[rstest::rstest]
    #[case(
        ProductDomainEventPayload::Created(created_payload()),
        "DOMAIN_CREATED"
    )]
    #[case(
        ProductDomainEventPayload::StateChanged(state_payload()),
        "DOMAIN_STATE_CHANGED"
    )]
    #[case(
        ProductDomainEventPayload::PriceChanged(price_payload()),
        "DOMAIN_PRICE_CHANGED"
    )]
    #[case(
        ProductDomainEventPayload::EstimatePriceChanged(estimate_payload()),
        "DOMAIN_ESTIMATE_PRICE_CHANGED"
    )]
    #[case(
        ProductDomainEventPayload::UrlChanged(url_payload()),
        "DOMAIN_URL_CHANGED"
    )]
    #[case(
        ProductDomainEventPayload::ImagesChanged(images_payload()),
        "DOMAIN_IMAGES_CHANGED"
    )]
    #[case(
        ProductDomainEventPayload::AuctionTimeChanged(auction_payload()),
        "DOMAIN_AUCTION_TIME_CHANGED"
    )]
    fn should_return_event_type_and_common_fields_for_all_domain_events(
        #[case] payload: ProductDomainEventPayload,
        #[case] event_type: &'static str,
    ) {
        let key = payload.key();

        assert_eq!(event_type, payload.event_type());
        assert_eq!(*payload.shop_id(), key.shop_id);
        assert_eq!(payload.shops_product_id(), &key.shops_product_id);
        assert_ne!(payload.shop_id(), payload.seller_id());
    }

    #[test]
    fn should_identify_state_price_and_downcast_variants() {
        let created = ProductDomainEventPayload::Created(created_payload());
        let state = ProductDomainEventPayload::StateChanged(state_payload());
        let price = ProductDomainEventPayload::PriceChanged(price_payload());

        assert!(created.as_created().is_some());
        assert!(created.as_state_changed().is_none());
        assert!(!created.is_price_event());
        assert!(!created.is_state_event());
        assert!(state.is_state_event());
        assert_eq!(Some(ProductState::Available), state.as_new_state());
        assert!(state.as_state_changed().is_some());
        assert!(price.is_price_event());
        assert!(price.as_price_changed().is_some());
        assert_eq!(None, price.as_new_state());
    }

    #[test]
    fn should_merge_native_prices_into_price_maps() {
        let payload = price_payload();

        assert_eq!(
            Some(&MonetaryAmount::from(100_u64)),
            payload.old_prices().get(&Currency::Eur)
        );
        assert_eq!(
            Some(&MonetaryAmount::from(110_u64)),
            payload.old_prices().get(&Currency::Usd)
        );
        assert_eq!(
            Some(&MonetaryAmount::from(200_u64)),
            payload.new_prices().get(&Currency::Eur)
        );
        assert_eq!(
            Some(&MonetaryAmount::from(220_u64)),
            payload.new_prices().get(&Currency::Usd)
        );
    }

    #[test]
    fn should_localize_created_to_requested_currency() {
        let view = ProductDomainEventPayload::Created(created_payload()).localized(&Currency::Usd);

        assert!(matches!(
            view,
            LocalizedProductDomainEventPayloadView::Created(payload)
                if payload.price == Some(price(110, Currency::Usd))
                    && payload.title.payload == Title::from("Bronze Vase")
        ));
    }

    #[test]
    fn should_localize_price_and_estimate_changes() {
        let price_view =
            ProductDomainEventPayload::PriceChanged(price_payload()).localized(&Currency::Usd);
        let estimate_view = ProductDomainEventPayload::EstimatePriceChanged(estimate_payload())
            .localized(&Currency::Usd);

        assert!(matches!(
            price_view,
            LocalizedProductDomainEventPayloadView::PriceChanged(payload)
                if payload.old_price == Some(price(110, Currency::Usd))
                    && payload.new_price == Some(price(220, Currency::Usd))
        ));
        assert!(matches!(
            estimate_view,
            LocalizedProductDomainEventPayloadView::EstimatePriceChanged(payload)
                if payload.price_estimate_min == Some(price(99, Currency::Usd))
                    && payload.price_estimate_max == Some(price(132, Currency::Usd))
        ));
    }

    #[test]
    fn should_localize_non_price_events() {
        let state =
            ProductDomainEventPayload::StateChanged(state_payload()).localized(&Currency::Eur);
        let url = ProductDomainEventPayload::UrlChanged(url_payload()).localized(&Currency::Eur);
        let images =
            ProductDomainEventPayload::ImagesChanged(images_payload()).localized(&Currency::Eur);
        let auction = ProductDomainEventPayload::AuctionTimeChanged(auction_payload())
            .localized(&Currency::Eur);

        assert!(matches!(
            state,
            LocalizedProductDomainEventPayloadView::StateChanged(_)
        ));
        assert!(matches!(
            url,
            LocalizedProductDomainEventPayloadView::UrlChanged(_)
        ));
        assert!(matches!(
            images,
            LocalizedProductDomainEventPayloadView::ImagesChanged(_)
        ));
        assert!(matches!(
            auction,
            LocalizedProductDomainEventPayloadView::AuctionTimeChanged(_)
        ));
    }
}
