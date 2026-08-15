use crate::description::Description;
use crate::product_image::ProductImage;
use crate::title::Title;
use common::fx_rate_id::FxRateId;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::Price;
use common::product_state::domain::ProductState;
use geo::core::address::{GeoAddress, StructuredAddress};
use indexmap::IndexSet;
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

#[derive(Debug, Clone, PartialEq)]
pub struct ProductCreatedDomainEventPayload {
    pub structured_address: Option<StructuredAddress>,
    pub geo_address: Option<GeoAddress>,
    pub title: Localized<Language, Title>,
    pub description: Option<Localized<Language, Description>>,
    pub price: Option<Price>,
    pub price_estimate_min: Option<Price>,
    pub price_estimate_max: Option<Price>,
    pub fx_rate_id: Option<FxRateId>,
    pub state: ProductState,
    pub url: Url,
    pub images: IndexSet<ProductImage>,
    pub auction_start: Option<OffsetDateTime>,
    pub auction_end: Option<OffsetDateTime>,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct ProductStateChangeDomainEventPayload {
    pub old_state: ProductState,
    pub new_state: ProductState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductPriceChangeDomainEventPayload {
    pub old_price: Option<Price>,
    pub new_price: Option<Price>,
    pub old_fx_rate_id: Option<FxRateId>,
    pub new_fx_rate_id: Option<FxRateId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductEstimatePriceChangeDomainEventPayload {
    pub old_price_estimate_min: Option<Price>,
    pub old_price_estimate_max: Option<Price>,
    pub new_price_estimate_min: Option<Price>,
    pub new_price_estimate_max: Option<Price>,
    pub old_fx_rate_id: Option<FxRateId>,
    pub new_fx_rate_id: Option<FxRateId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductUrlChangeDomainEventPayload {
    pub url: Url,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductImagesChangeDomainEventPayload {
    pub images: IndexSet<ProductImage>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductAuctionTimeChangeDomainEventPayload {
    pub auction_start: Option<OffsetDateTime>,
    pub auction_end: Option<OffsetDateTime>,
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
                structured_address: config.fake_with_rng(rng),
                geo_address: config.fake_with_rng(rng),
                title: config.fake_with_rng(rng),
                description: config.fake_with_rng(rng),
                price: config.fake_with_rng(rng),
                price_estimate_min: config.fake_with_rng(rng),
                price_estimate_max: config.fake_with_rng(rng),
                fx_rate_id: Some(FxRateId::new()),
                state: config.fake_with_rng(rng),
                url: test_url("https://www.example.com/product"),
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
                url: test_url("https://www.example.com/product"),
            }
        }
    }

    impl Dummy<Faker> for ProductImagesChangeDomainEventPayload {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductImagesChangeDomainEventPayload {
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

    fn created_payload() -> ProductCreatedDomainEventPayload {
        ProductCreatedDomainEventPayload {
            structured_address: None,
            geo_address: None,
            title: Localized::new(Language::De, Title::from("Bronze Vase")),
            description: Some(Localized::new(Language::De, Description::from("Alt"))),
            price: Some(price(100, Currency::Eur)),
            price_estimate_min: Some(price(90, Currency::Eur)),
            price_estimate_max: Some(price(120, Currency::Eur)),
            fx_rate_id: Some(FxRateId::new()),
            state: ProductState::Listed,
            url: url("product"),
            images: IndexSet::new(),
            auction_start: Some(OffsetDateTime::UNIX_EPOCH),
            auction_end: None,
        }
    }

    fn state_payload() -> ProductStateChangeDomainEventPayload {
        ProductStateChangeDomainEventPayload {
            old_state: ProductState::Listed,
            new_state: ProductState::Available,
        }
    }

    fn price_payload() -> ProductPriceChangeDomainEventPayload {
        ProductPriceChangeDomainEventPayload {
            old_price: Some(price(100, Currency::Eur)),
            new_price: Some(price(200, Currency::Eur)),
            old_fx_rate_id: Some(FxRateId::new()),
            new_fx_rate_id: Some(FxRateId::new()),
        }
    }

    fn estimate_payload() -> ProductEstimatePriceChangeDomainEventPayload {
        ProductEstimatePriceChangeDomainEventPayload {
            old_price_estimate_min: Some(price(90, Currency::Eur)),
            old_price_estimate_max: Some(price(120, Currency::Eur)),
            new_price_estimate_min: Some(price(99, Currency::Usd)),
            new_price_estimate_max: Some(price(132, Currency::Usd)),
            old_fx_rate_id: Some(FxRateId::new()),
            new_fx_rate_id: Some(FxRateId::new()),
        }
    }

    fn url_payload() -> ProductUrlChangeDomainEventPayload {
        ProductUrlChangeDomainEventPayload {
            url: url("product"),
        }
    }

    fn images_payload() -> ProductImagesChangeDomainEventPayload {
        ProductImagesChangeDomainEventPayload {
            images: IndexSet::new(),
        }
    }

    fn auction_payload() -> ProductAuctionTimeChangeDomainEventPayload {
        ProductAuctionTimeChangeDomainEventPayload {
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
    fn should_return_event_type_for_all_domain_events(
        #[case] payload: ProductDomainEventPayload,
        #[case] event_type: &'static str,
    ) {
        assert_eq!(event_type, payload.event_type());
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
