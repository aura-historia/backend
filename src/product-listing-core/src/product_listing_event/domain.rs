use crate::description::Description;
use crate::product_listing_image::ProductListingImage;
use crate::product_state::ProductState;
use crate::title::Title;
use geo::core::address::{GeoAddress, StructuredAddress};
use indexmap::IndexSet;
use localization::Language;
use localization::Localized;
use money::Price;
use time::OffsetDateTime;
use url::Url;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ProductListingDomainEventPayload {
    Created(ProductListingCreatedDomainEventPayload),
    StateChanged(ProductListingStateChangeDomainEventPayload),
    PriceChanged(ProductListingPriceChangeDomainEventPayload),
    EstimatePriceChanged(ProductListingEstimatePriceChangeDomainEventPayload),
    UrlChanged(ProductListingUrlChangeDomainEventPayload),
    ImagesChanged(ProductListingImagesChangeDomainEventPayload),
    AuctionTimeChanged(ProductListingAuctionTimeChangeDomainEventPayload),
}

impl ProductListingDomainEventPayload {
    pub fn event_type(&self) -> &'static str {
        match self {
            ProductListingDomainEventPayload::Created(_) => "DOMAIN_CREATED",
            ProductListingDomainEventPayload::StateChanged(_) => "DOMAIN_STATE_CHANGED",
            ProductListingDomainEventPayload::PriceChanged(_) => "DOMAIN_PRICE_CHANGED",
            ProductListingDomainEventPayload::EstimatePriceChanged(_) => {
                "DOMAIN_ESTIMATE_PRICE_CHANGED"
            }
            ProductListingDomainEventPayload::UrlChanged(_) => "DOMAIN_URL_CHANGED",
            ProductListingDomainEventPayload::ImagesChanged(_) => "DOMAIN_IMAGES_CHANGED",
            ProductListingDomainEventPayload::AuctionTimeChanged(_) => {
                "DOMAIN_AUCTION_TIME_CHANGED"
            }
        }
    }

    pub fn is_price_event(&self) -> bool {
        matches!(self, ProductListingDomainEventPayload::PriceChanged(_))
    }

    pub fn is_state_event(&self) -> bool {
        matches!(self, ProductListingDomainEventPayload::StateChanged(_))
    }

    pub fn as_created(&self) -> Option<&ProductListingCreatedDomainEventPayload> {
        match self {
            ProductListingDomainEventPayload::Created(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_state_changed(&self) -> Option<&ProductListingStateChangeDomainEventPayload> {
        match self {
            ProductListingDomainEventPayload::StateChanged(payload) => Some(payload),
            _ => None,
        }
    }

    pub fn as_new_state(&self) -> Option<ProductState> {
        match self {
            ProductListingDomainEventPayload::StateChanged(payload) => Some(payload.new_state),
            _ => None,
        }
    }

    pub fn as_price_changed(&self) -> Option<&ProductListingPriceChangeDomainEventPayload> {
        match self {
            ProductListingDomainEventPayload::PriceChanged(payload) => Some(payload),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingCreatedDomainEventPayload {
    pub structured_address: Option<StructuredAddress>,
    pub geo_address: Option<GeoAddress>,
    pub title: Localized<Language, Title>,
    pub description: Option<Localized<Language, Description>>,
    pub price: Option<Price>,
    pub price_estimate_min: Option<Price>,
    pub price_estimate_max: Option<Price>,
    pub state: ProductState,
    pub url: Url,
    pub images: IndexSet<ProductListingImage>,
    pub auction_start: Option<OffsetDateTime>,
    pub auction_end: Option<OffsetDateTime>,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingStateChangeDomainEventPayload {
    pub old_state: ProductState,
    pub new_state: ProductState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingPriceChangeDomainEventPayload {
    pub old_price: Option<Price>,
    pub new_price: Option<Price>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingEstimatePriceChangeDomainEventPayload {
    pub old_price_estimate_min: Option<Price>,
    pub old_price_estimate_max: Option<Price>,
    pub new_price_estimate_min: Option<Price>,
    pub new_price_estimate_max: Option<Price>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingUrlChangeDomainEventPayload {
    pub url: Url,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingImagesChangeDomainEventPayload {
    pub images: IndexSet<ProductListingImage>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingAuctionTimeChangeDomainEventPayload {
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

    impl Dummy<Faker> for ProductListingCreatedDomainEventPayload {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductListingCreatedDomainEventPayload {
                structured_address: config.fake_with_rng(rng),
                geo_address: config.fake_with_rng(rng),
                title: config.fake_with_rng(rng),
                description: config.fake_with_rng(rng),
                price: config.fake_with_rng(rng),
                price_estimate_min: config.fake_with_rng(rng),
                price_estimate_max: config.fake_with_rng(rng),
                state: config.fake_with_rng(rng),
                url: test_url("https://www.example.com/product"),
                images: config
                    .fake_with_rng::<Vec<ProductListingImage>, _>(rng)
                    .into_iter()
                    .collect(),
                auction_start: config.fake_with_rng(rng),
                auction_end: config.fake_with_rng(rng),
            }
        }
    }

    impl Dummy<Faker> for ProductListingPriceChangeDomainEventPayload {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductListingPriceChangeDomainEventPayload {
                old_price: config.fake_with_rng(rng),
                new_price: config.fake_with_rng(rng),
            }
        }
    }

    impl Dummy<Faker> for ProductListingEstimatePriceChangeDomainEventPayload {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductListingEstimatePriceChangeDomainEventPayload {
                old_price_estimate_min: config.fake_with_rng(rng),
                old_price_estimate_max: config.fake_with_rng(rng),
                new_price_estimate_min: config.fake_with_rng(rng),
                new_price_estimate_max: config.fake_with_rng(rng),
            }
        }
    }

    impl Dummy<Faker> for ProductListingUrlChangeDomainEventPayload {
        fn dummy_with_rng<R: RngExt + ?Sized>(_config: &Faker, _rng: &mut R) -> Self {
            ProductListingUrlChangeDomainEventPayload {
                url: test_url("https://www.example.com/product"),
            }
        }
    }

    impl Dummy<Faker> for ProductListingImagesChangeDomainEventPayload {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductListingImagesChangeDomainEventPayload {
                images: config
                    .fake_with_rng::<Vec<ProductListingImage>, _>(rng)
                    .into_iter()
                    .collect(),
            }
        }
    }

    impl Dummy<Faker> for ProductListingAuctionTimeChangeDomainEventPayload {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductListingAuctionTimeChangeDomainEventPayload {
                auction_start: config.fake_with_rng(rng),
                auction_end: config.fake_with_rng(rng),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use money::Currency;
    use money::MonetaryAmount;

    fn price(amount: u64, currency: Currency) -> Price {
        Price::new(MonetaryAmount::from(amount), currency)
    }

    fn url(path: &str) -> Url {
        match Url::parse(&format!("https://shop.example/{path}")) {
            Ok(url) => url,
            Err(error) => panic!("invalid test URL: {error}"),
        }
    }

    fn created_payload() -> ProductListingCreatedDomainEventPayload {
        ProductListingCreatedDomainEventPayload {
            structured_address: None,
            geo_address: None,
            title: Localized::new(Language::De, Title::from("Bronze Vase")),
            description: Some(Localized::new(Language::De, Description::from("Alt"))),
            price: Some(price(100, Currency::Eur)),
            price_estimate_min: Some(price(90, Currency::Eur)),
            price_estimate_max: Some(price(120, Currency::Eur)),
            state: ProductState::Listed,
            url: url("product"),
            images: IndexSet::new(),
            auction_start: Some(OffsetDateTime::UNIX_EPOCH),
            auction_end: None,
        }
    }

    fn state_payload() -> ProductListingStateChangeDomainEventPayload {
        ProductListingStateChangeDomainEventPayload {
            old_state: ProductState::Listed,
            new_state: ProductState::Available,
        }
    }

    fn price_payload() -> ProductListingPriceChangeDomainEventPayload {
        ProductListingPriceChangeDomainEventPayload {
            old_price: Some(price(100, Currency::Eur)),
            new_price: Some(price(200, Currency::Eur)),
        }
    }

    fn estimate_payload() -> ProductListingEstimatePriceChangeDomainEventPayload {
        ProductListingEstimatePriceChangeDomainEventPayload {
            old_price_estimate_min: Some(price(90, Currency::Eur)),
            old_price_estimate_max: Some(price(120, Currency::Eur)),
            new_price_estimate_min: Some(price(99, Currency::Usd)),
            new_price_estimate_max: Some(price(132, Currency::Usd)),
        }
    }

    fn url_payload() -> ProductListingUrlChangeDomainEventPayload {
        ProductListingUrlChangeDomainEventPayload {
            url: url("product"),
        }
    }

    fn images_payload() -> ProductListingImagesChangeDomainEventPayload {
        ProductListingImagesChangeDomainEventPayload {
            images: IndexSet::new(),
        }
    }

    fn auction_payload() -> ProductListingAuctionTimeChangeDomainEventPayload {
        ProductListingAuctionTimeChangeDomainEventPayload {
            auction_start: Some(OffsetDateTime::UNIX_EPOCH),
            auction_end: None,
        }
    }

    #[rstest::rstest]
    #[case(
        ProductListingDomainEventPayload::Created(created_payload()),
        "DOMAIN_CREATED"
    )]
    #[case(
        ProductListingDomainEventPayload::StateChanged(state_payload()),
        "DOMAIN_STATE_CHANGED"
    )]
    #[case(
        ProductListingDomainEventPayload::PriceChanged(price_payload()),
        "DOMAIN_PRICE_CHANGED"
    )]
    #[case(
        ProductListingDomainEventPayload::EstimatePriceChanged(estimate_payload()),
        "DOMAIN_ESTIMATE_PRICE_CHANGED"
    )]
    #[case(
        ProductListingDomainEventPayload::UrlChanged(url_payload()),
        "DOMAIN_URL_CHANGED"
    )]
    #[case(
        ProductListingDomainEventPayload::ImagesChanged(images_payload()),
        "DOMAIN_IMAGES_CHANGED"
    )]
    #[case(
        ProductListingDomainEventPayload::AuctionTimeChanged(auction_payload()),
        "DOMAIN_AUCTION_TIME_CHANGED"
    )]
    fn should_return_event_type_for_all_domain_events(
        #[case] payload: ProductListingDomainEventPayload,
        #[case] event_type: &'static str,
    ) {
        assert_eq!(event_type, payload.event_type());
    }

    #[test]
    fn should_identify_state_price_and_downcast_variants() {
        let created = ProductListingDomainEventPayload::Created(created_payload());
        let state = ProductListingDomainEventPayload::StateChanged(state_payload());
        let price = ProductListingDomainEventPayload::PriceChanged(price_payload());

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
    fn should_keep_source_pricing_snapshots_for_price_events() {
        let price_change = price_payload();
        let estimate_change = estimate_payload();

        assert_eq!(Some(price(100, Currency::Eur)), price_change.old_price);
        assert_eq!(Some(price(200, Currency::Eur)), price_change.new_price);
        assert_eq!(
            Some(price(90, Currency::Eur)),
            estimate_change.old_price_estimate_min
        );
        assert_eq!(
            Some(price(132, Currency::Usd)),
            estimate_change.new_price_estimate_max
        );
    }

    #[test]
    fn should_keep_source_pricing_in_created_snapshot() {
        let payload = created_payload();

        assert_eq!(Some(price(100, Currency::Eur)), payload.price);
        assert_eq!(Some(price(90, Currency::Eur)), payload.price_estimate_min);
        assert_eq!(Some(price(120, Currency::Eur)), payload.price_estimate_max);
    }
}
