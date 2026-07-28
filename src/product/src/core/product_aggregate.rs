use crate::core::description::Description;
use crate::core::fx_rate_id::FxRateId;
use crate::core::product_image::ProductImage;
use crate::core::title::Title;
use common::change_outcome::ChangeOutcome;
use common::event::Event;
use common::event_id::EventId;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::Price;
use common::product_id::ProductId;
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_slug_id::ProductSlugId;
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use geo::core::address::{GeoAddress, StructuredAddress};
use indexmap::IndexSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct Product {
    id: ProductId,
    slug_id: ProductSlugId,
    shop_id: ShopId,
    seller_id: ShopId,
    shops_product_id: ShopsProductId,
    address: ProductAddress,
    title: Option<Localized<Language, Title>>,
    description: Option<Localized<Language, Description>>,
    pricing: ProductPricing,
    state: ProductState,
    lifecycle: ProductLifecycle,
    url: Url,
    images: IndexSet<ProductImage>,
    auction: ProductAuction,
    pending_events: Vec<ProductDomainEvent>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewProduct {
    pub id: ProductId,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub address: ProductAddress,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub pricing: ProductPricing,
    pub state: ProductState,
    pub url: Url,
    pub images: IndexSet<ProductImage>,
    pub auction: ProductAuction,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProductStateSnapshot {
    pub id: ProductId,
    pub slug_id: ProductSlugId,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub address: ProductAddress,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub pricing: ProductPricing,
    pub state: ProductState,
    pub lifecycle: ProductLifecycle,
    pub url: Url,
    pub images: IndexSet<ProductImage>,
    pub auction: ProductAuction,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProductAddress {
    pub structured: Option<StructuredAddress>,
    pub geo: Option<GeoAddress>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ProductPricing {
    pub native_price: Option<Price>,
    pub native_price_estimate_min: Option<Price>,
    pub native_price_estimate_max: Option<Price>,
    pub fx_rate_id: Option<FxRateId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ProductAuction {
    pub start: Option<OffsetDateTime>,
    pub end: Option<OffsetDateTime>,
}

pub type ProductDomainEvent = Event<ProductId, ProductDomainEventPayload>;

#[derive(Debug, Clone, PartialEq)]
pub enum ProductDomainEventPayload {
    Created(Box<ProductCreated>),
    StateChanged(ProductStateChanged),
    AddressChanged(ProductAddressChanged),
    PriceChanged(ProductPriceChanged),
    UrlChanged(ProductUrlChanged),
    ImagesChanged(Box<ProductImagesChanged>),
    AuctionChanged(ProductAuctionChanged),
    Deleted(ProductDeleted),
}

impl ProductDomainEventPayload {
    pub fn event_type(&self) -> &'static str {
        match self {
            ProductDomainEventPayload::Created(_) => "PRODUCT_CREATED",
            ProductDomainEventPayload::StateChanged(_) => "PRODUCT_STATE_CHANGED",
            ProductDomainEventPayload::AddressChanged(_) => "PRODUCT_ADDRESS_CHANGED",
            ProductDomainEventPayload::PriceChanged(_) => "PRODUCT_PRICE_CHANGED",
            ProductDomainEventPayload::UrlChanged(_) => "PRODUCT_URL_CHANGED",
            ProductDomainEventPayload::ImagesChanged(_) => "PRODUCT_IMAGES_CHANGED",
            ProductDomainEventPayload::AuctionChanged(_) => "PRODUCT_AUCTION_CHANGED",
            ProductDomainEventPayload::Deleted(_) => "PRODUCT_DELETED",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductCreated {
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub address: ProductAddress,
    pub pricing: ProductPricing,
    pub state: ProductState,
    pub url: Url,
    pub images: IndexSet<ProductImage>,
    pub auction: ProductAuction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductStateChanged {
    pub old_state: ProductState,
    pub new_state: ProductState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductAddressChanged {
    pub address: ProductAddress,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductPriceChanged {
    pub old_pricing: ProductPricing,
    pub new_pricing: ProductPricing,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductUrlChanged {
    pub old_url: Url,
    pub new_url: Url,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductImagesChanged {
    pub images: IndexSet<ProductImage>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductAuctionChanged {
    pub auction: ProductAuction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductDeleted {
    pub old_lifecycle: ProductLifecycle,
    pub new_lifecycle: ProductLifecycle,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RehydrateProductError {
    #[error("product geo latitude out of range")]
    GeoLatitudeOutOfRange,
    #[error("product geo longitude out of range")]
    GeoLongitudeOutOfRange,
}

impl Product {
    pub fn create(input: NewProduct) -> Result<Self, RehydrateProductError> {
        let slug_id = product_slug_id(input.id, input.title.as_ref());
        let mut product = Self::rehydrate(ProductStateSnapshot {
            id: input.id,
            slug_id,
            shop_id: input.shop_id,
            seller_id: input.seller_id,
            shops_product_id: input.shops_product_id,
            address: input.address.clone(),
            title: input.title.clone(),
            description: input.description.clone(),
            pricing: input.pricing,
            state: input.state,
            lifecycle: ProductLifecycle::Active,
            url: input.url.clone(),
            images: input.images.clone(),
            auction: input.auction,
        })?;

        product.push_event(ProductDomainEventPayload::Created(Box::new(
            ProductCreated {
                title: input.title,
                description: input.description,
                address: input.address,
                pricing: input.pricing,
                state: input.state,
                url: input.url,
                images: input.images,
                auction: input.auction,
            },
        )));

        Ok(product)
    }

    #[allow(dead_code)]
    pub(crate) fn rehydrate(state: ProductStateSnapshot) -> Result<Self, RehydrateProductError> {
        validate_geo_address(state.address.geo)?;

        Ok(Self {
            id: state.id,
            slug_id: state.slug_id,
            shop_id: state.shop_id,
            seller_id: state.seller_id,
            shops_product_id: state.shops_product_id,
            address: state.address,
            title: state.title,
            description: state.description,
            pricing: state.pricing,
            state: state.state,
            lifecycle: state.lifecycle,
            url: state.url,
            images: state.images,
            auction: state.auction,
            pending_events: Vec::new(),
        })
    }

    pub fn replace_address(&mut self, address: ProductAddress) -> ChangeOutcome {
        if replace_if_changed(&mut self.address, address.clone()) == ChangeOutcome::Unchanged {
            return ChangeOutcome::Unchanged;
        }

        self.push_event(ProductDomainEventPayload::AddressChanged(
            ProductAddressChanged { address },
        ));
        ChangeOutcome::Changed
    }

    pub fn change_state(&mut self, new_state: ProductState) -> ChangeOutcome {
        if self.state == new_state {
            return ChangeOutcome::Unchanged;
        }

        let old_state = self.state;
        self.state = new_state;
        self.push_event(ProductDomainEventPayload::StateChanged(
            ProductStateChanged {
                old_state,
                new_state,
            },
        ));
        ChangeOutcome::Changed
    }

    pub fn replace_pricing(&mut self, pricing: ProductPricing) -> ChangeOutcome {
        if self.pricing == pricing {
            return ChangeOutcome::Unchanged;
        }

        let old_pricing = self.pricing;
        self.pricing = pricing;
        self.push_event(ProductDomainEventPayload::PriceChanged(
            ProductPriceChanged {
                old_pricing,
                new_pricing: pricing,
            },
        ));
        ChangeOutcome::Changed
    }

    pub fn change_url(&mut self, url: Url) -> ChangeOutcome {
        if self.url == url {
            return ChangeOutcome::Unchanged;
        }

        let old_url = self.url.clone();
        self.url = url.clone();
        self.push_event(ProductDomainEventPayload::UrlChanged(ProductUrlChanged {
            old_url,
            new_url: url,
        }));
        ChangeOutcome::Changed
    }

    pub fn replace_images(&mut self, images: IndexSet<ProductImage>) -> ChangeOutcome {
        if self.images == images {
            return ChangeOutcome::Unchanged;
        }

        self.images = images.clone();
        self.push_event(ProductDomainEventPayload::ImagesChanged(Box::new(
            ProductImagesChanged { images },
        )));
        ChangeOutcome::Changed
    }

    pub fn replace_auction(&mut self, auction: ProductAuction) -> ChangeOutcome {
        if self.auction == auction {
            return ChangeOutcome::Unchanged;
        }

        self.auction = auction;
        self.push_event(ProductDomainEventPayload::AuctionChanged(
            ProductAuctionChanged { auction },
        ));
        ChangeOutcome::Changed
    }

    pub fn delete(&mut self) -> ChangeOutcome {
        if self.lifecycle == ProductLifecycle::Deleted {
            return ChangeOutcome::Unchanged;
        }

        let old_lifecycle = self.lifecycle;
        self.lifecycle = ProductLifecycle::Deleted;
        self.push_event(ProductDomainEventPayload::Deleted(ProductDeleted {
            old_lifecycle,
            new_lifecycle: ProductLifecycle::Deleted,
        }));
        ChangeOutcome::Changed
    }

    pub fn pending_events(&self) -> &[ProductDomainEvent] {
        &self.pending_events
    }

    pub fn take_pending_events(&mut self) -> Vec<ProductDomainEvent> {
        std::mem::take(&mut self.pending_events)
    }

    pub fn id(&self) -> ProductId {
        self.id
    }

    pub fn slug_id(&self) -> &ProductSlugId {
        &self.slug_id
    }

    pub fn shop_id(&self) -> ShopId {
        self.shop_id
    }

    pub fn seller_id(&self) -> ShopId {
        self.seller_id
    }

    pub fn shops_product_id(&self) -> &ShopsProductId {
        &self.shops_product_id
    }

    pub fn address(&self) -> ProductAddress {
        self.address.clone()
    }

    pub fn title(&self) -> Option<&Localized<Language, Title>> {
        self.title.as_ref()
    }

    pub fn description(&self) -> Option<&Localized<Language, Description>> {
        self.description.as_ref()
    }

    pub fn pricing(&self) -> ProductPricing {
        self.pricing
    }

    pub fn state(&self) -> ProductState {
        self.state
    }

    pub fn lifecycle(&self) -> ProductLifecycle {
        self.lifecycle
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn view_url(&self) -> Url {
        common::utm::append_utm_params(self.url.clone())
    }

    pub fn images(&self) -> &IndexSet<ProductImage> {
        &self.images
    }

    pub fn auction(&self) -> ProductAuction {
        self.auction
    }

    fn push_event(&mut self, payload: ProductDomainEventPayload) {
        self.pending_events.push(Event {
            aggregate_id: self.id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload,
        });
    }
}

fn product_slug_id(
    product_id: ProductId,
    title: Option<&Localized<Language, Title>>,
) -> ProductSlugId {
    title
        .map(|title| ProductSlugId::from(title.payload.as_ref()))
        .unwrap_or_else(|| ProductSlugId::from(product_id.to_string()))
}

fn validate_geo_address(geo_address: Option<GeoAddress>) -> Result<(), RehydrateProductError> {
    if let Some(address) = geo_address {
        if !(-90.0..=90.0).contains(&address.lat) {
            return Err(RehydrateProductError::GeoLatitudeOutOfRange);
        }
        if !(-180.0..=180.0).contains(&address.lon) {
            return Err(RehydrateProductError::GeoLongitudeOutOfRange);
        }
    }

    Ok(())
}

fn replace_if_changed<T: PartialEq>(target: &mut T, value: T) -> ChangeOutcome {
    if *target == value {
        ChangeOutcome::Unchanged
    } else {
        *target = value;
        ChangeOutcome::Changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::currency::domain::Currency;
    use common::price::domain::MonetaryAmount;

    fn test_url() -> Url {
        match Url::parse("https://shop.example/products/1") {
            Ok(url) => url,
            Err(error) => panic!("invalid test URL: {error}"),
        }
    }

    fn new_product() -> NewProduct {
        NewProduct {
            id: ProductId::new(),
            shop_id: ShopId::new(),
            seller_id: ShopId::new(),
            shops_product_id: ShopsProductId::new(),
            address: ProductAddress::default(),
            title: Some(Localized::new(Language::En, Title::from("Bronze vase"))),
            description: None,
            pricing: ProductPricing {
                native_price: Some(Price::new(MonetaryAmount::from(1_500_u64), Currency::Eur)),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                fx_rate_id: Some(FxRateId::new()),
            },
            state: ProductState::Listed,
            url: test_url(),
            images: IndexSet::new(),
            auction: ProductAuction::default(),
        }
    }

    fn created_product() -> Product {
        match Product::create(new_product()) {
            Ok(product) => product,
            Err(error) => panic!("create failed: {error}"),
        }
    }

    #[test]
    fn should_keep_created_event_as_pending_when_product_created() {
        let product = created_product();

        assert!(matches!(
            product.pending_events(),
            [ProductDomainEvent {
                payload: ProductDomainEventPayload::Created(_),
                ..
            }]
        ));
    }

    #[test]
    fn should_allow_product_without_title() {
        let mut input = new_product();
        input.title = None;

        let product = Product::create(input);

        assert!(matches!(product, Ok(ref product) if product.title().is_none()));
    }

    #[test]
    fn should_rehydrate_without_pending_events() {
        let input = new_product();
        let product = Product::rehydrate(ProductStateSnapshot {
            id: input.id,
            slug_id: product_slug_id(input.id, input.title.as_ref()),
            shop_id: input.shop_id,
            seller_id: input.seller_id,
            shops_product_id: input.shops_product_id,
            address: input.address,
            title: input.title,
            description: input.description,
            pricing: input.pricing,
            state: input.state,
            lifecycle: ProductLifecycle::Active,
            url: input.url,
            images: input.images,
            auction: input.auction,
        });

        assert!(matches!(product, Ok(ref product) if product.pending_events().is_empty()));
    }

    #[test]
    fn should_not_emit_event_when_state_unchanged() {
        let mut product = created_product();
        product.take_pending_events();

        let outcome = product.change_state(ProductState::Listed);

        assert_eq!(ChangeOutcome::Unchanged, outcome);
        assert!(product.pending_events().is_empty());
    }

    #[test]
    fn should_emit_event_when_state_changes() {
        let mut product = created_product();
        product.take_pending_events();

        let outcome = product.change_state(ProductState::Available);

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert!(matches!(
            product.pending_events(),
            [ProductDomainEvent {
                payload: ProductDomainEventPayload::StateChanged(ProductStateChanged {
                    old_state: ProductState::Listed,
                    new_state: ProductState::Available,
                }),
                ..
            }]
        ));
        assert_eq!(ProductState::Available, product.state());
    }

    #[test]
    fn should_take_pending_events() {
        let mut product = created_product();

        let events = product.take_pending_events();

        assert_eq!(1, events.len());
        assert!(product.pending_events().is_empty());
    }

    #[test]
    fn should_reject_invalid_geo_when_rehydrating() {
        let input = new_product();
        let result = Product::rehydrate(ProductStateSnapshot {
            id: input.id,
            slug_id: product_slug_id(input.id, input.title.as_ref()),
            shop_id: input.shop_id,
            seller_id: input.seller_id,
            shops_product_id: input.shops_product_id,
            address: ProductAddress {
                structured: None,
                geo: Some(GeoAddress {
                    lat: 91.0,
                    lon: 0.0,
                }),
            },
            title: input.title,
            description: input.description,
            pricing: input.pricing,
            state: input.state,
            lifecycle: ProductLifecycle::Active,
            url: input.url,
            images: input.images,
            auction: input.auction,
        });

        assert_eq!(Err(RehydrateProductError::GeoLatitudeOutOfRange), result);
    }
}
