use crate::description::Description;
use crate::fx_rate_id::FxRateId;
use crate::product_image::ProductImage;
use crate::title::Title;
use common::has_key::HasKey;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::Price;
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
    pub price: Option<Price>,
    pub price_estimate_min: Option<Price>,
    pub price_estimate_max: Option<Price>,
    pub fx_rate_id: Option<FxRateId>,
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

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
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
pub struct ProductPriceChangeDomainEventPayload {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub old_price: Option<Price>,
    pub new_price: Option<Price>,
    pub old_fx_rate_id: Option<FxRateId>,
    pub new_fx_rate_id: Option<FxRateId>,
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

#[derive(Debug, Clone, PartialEq)]
pub struct ProductEstimatePriceChangeDomainEventPayload {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub old_price_estimate_min: Option<Price>,
    pub old_price_estimate_max: Option<Price>,
    pub new_price_estimate_min: Option<Price>,
    pub new_price_estimate_max: Option<Price>,
    pub old_fx_rate_id: Option<FxRateId>,
    pub new_fx_rate_id: Option<FxRateId>,
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

    fn test_url(path: &str) -> Url {
        match Url::parse(path) {
            Ok(url) => url,
            Err(error) => panic!("invalid faker URL: {error}"),
        }
    }

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
                price: config.fake_with_rng(rng),
                price_estimate_min: config.fake_with_rng(rng),
                price_estimate_max: config.fake_with_rng(rng),
                fx_rate_id: Some(FxRateId::new()),
                state: config.fake_with_rng(rng),
                url: test_url("https://www.example.com/product"),
                view_url: test_url("https://www.example.com/product?utm_source=aura-historia"),
                images: config
                    .fake_with_rng::<Vec<ProductImage>, _>(rng)
                    .into_iter()
                    .collect(),
                auction_start: config.fake_with_rng(rng),
                auction_end: config.fake_with_rng(rng),
            }
        }
    }

    impl Dummy<Faker> for ProductPriceChangeDomainEventPayload {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductPriceChangeDomainEventPayload {
                shop_id: config.fake_with_rng(rng),
                seller_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                old_price: config.fake_with_rng(rng),
                new_price: config.fake_with_rng(rng),
                old_fx_rate_id: Some(FxRateId::new()),
                new_fx_rate_id: Some(FxRateId::new()),
            }
        }
    }

    impl Dummy<Faker> for ProductEstimatePriceChangeDomainEventPayload {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductEstimatePriceChangeDomainEventPayload {
                shop_id: config.fake_with_rng(rng),
                seller_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                old_price_estimate_min: config.fake_with_rng(rng),
                old_price_estimate_max: config.fake_with_rng(rng),
                new_price_estimate_min: config.fake_with_rng(rng),
                new_price_estimate_max: config.fake_with_rng(rng),
                old_fx_rate_id: Some(FxRateId::new()),
                new_fx_rate_id: Some(FxRateId::new()),
            }
        }
    }

    impl Dummy<Faker> for ProductUrlChangeDomainEventPayload {
        fn dummy_with_rng<R: RngExt + ?Sized>(_config: &Faker, _rng: &mut R) -> Self {
            ProductUrlChangeDomainEventPayload {
                shop_id: ShopId::new(),
                seller_id: ShopId::new(),
                shops_product_id: ShopsProductId::new(),
                url: test_url("https://www.example.com/product"),
                view_url: test_url("https://www.example.com/product?utm_source=aura-historia"),
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

    impl Dummy<Faker> for ProductAuctionTimeChangeDomainEventPayload {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductAuctionTimeChangeDomainEventPayload {
                shop_id: config.fake_with_rng(rng),
                seller_id: config.fake_with_rng(rng),
                shops_product_id: config.fake_with_rng(rng),
                auction_start: config.fake_with_rng(rng),
                auction_end: config.fake_with_rng(rng),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::currency::domain::Currency;
    use common::price::domain::MonetaryAmount;

    fn price(amount: u64, currency: Currency) -> Price {
        Price::new(MonetaryAmount::from(amount), currency)
    }

    fn url(path: &str) -> Url {
        match Url::parse(&format!("https://shop.example/{path}")) {
            Ok(url) => url,
            Err(error) => panic!("invalid test URL: {error}"),
        }
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
            price: Some(price(100, Currency::Eur)),
            price_estimate_min: Some(price(90, Currency::Eur)),
            price_estimate_max: Some(price(120, Currency::Eur)),
            fx_rate_id: Some(FxRateId::new()),
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
            old_price: Some(price(100, Currency::Eur)),
            new_price: Some(price(200, Currency::Eur)),
            old_fx_rate_id: Some(FxRateId::new()),
            new_fx_rate_id: Some(FxRateId::new()),
        }
    }

    fn estimate_payload() -> ProductEstimatePriceChangeDomainEventPayload {
        let (shop_id, seller_id, shops_product_id) = ids();
        ProductEstimatePriceChangeDomainEventPayload {
            shop_id,
            seller_id,
            shops_product_id,
            old_price_estimate_min: Some(price(90, Currency::Eur)),
            old_price_estimate_max: Some(price(120, Currency::Eur)),
            new_price_estimate_min: Some(price(99, Currency::Usd)),
            new_price_estimate_max: Some(price(132, Currency::Usd)),
            old_fx_rate_id: Some(FxRateId::new()),
            new_fx_rate_id: Some(FxRateId::new()),
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
    fn should_keep_pricing_and_fx_rate_snapshots_for_price_events() {
        let price_change = price_payload();
        let estimate_change = estimate_payload();

        assert_eq!(Some(price(100, Currency::Eur)), price_change.old_price);
        assert_eq!(Some(price(200, Currency::Eur)), price_change.new_price);
        assert_ne!(price_change.old_fx_rate_id, price_change.new_fx_rate_id);
        assert_eq!(
            Some(price(90, Currency::Eur)),
            estimate_change.old_price_estimate_min
        );
        assert_eq!(
            Some(price(132, Currency::Usd)),
            estimate_change.new_price_estimate_max
        );
        assert_ne!(
            estimate_change.old_fx_rate_id,
            estimate_change.new_fx_rate_id
        );
    }

    #[test]
    fn should_keep_pricing_and_fx_rate_in_created_snapshot() {
        let payload = created_payload();

        assert_eq!(Some(price(100, Currency::Eur)), payload.price);
        assert_eq!(Some(price(90, Currency::Eur)), payload.price_estimate_min);
        assert_eq!(Some(price(120, Currency::Eur)), payload.price_estimate_max);
        assert!(payload.fx_rate_id.is_some());
    }
}
