use crate::description::Description;
use crate::product_id::ProductId;
use crate::product_image::ProductImage;
use crate::product_lifecycle::ProductLifecycle;
use crate::product_slug_id::ProductSlugId;
use crate::product_state::ProductState;
use crate::shops_product_id::ShopsProductId;
use crate::title::Title;
use domain_primitives::{change_outcome::ChangeOutcome, event::Event, event_id::EventId};
use fxrate_core::FxRateId;
use geo::core::address::{GeoAddress, StructuredAddress};
use indexmap::IndexSet;
use localization::Language;
use localization::Localized;
use money::Price;
use shop_core::shop_id::ShopId;
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
    sale_valuation: Option<ProductSaleValuation>,
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
    pub sale_valuation: Option<ProductSaleValuation>,
    pub state: ProductState,
    pub url: Url,
    pub images: IndexSet<ProductImage>,
    pub auction: ProductAuction,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedProductState {
    pub id: ProductId,
    pub slug_id: ProductSlugId,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub address: ProductAddress,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub pricing: ProductPricing,
    pub sale_valuation: Option<ProductSaleValuation>,
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
    pub price: Option<Price>,
    pub price_estimate_min: Option<Price>,
    pub price_estimate_max: Option<Price>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::EnumIter)]
pub enum ProductPriceValuationBasis {
    Current,
    Event,
    Sale,
}

impl ProductPriceValuationBasis {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Current => "CURRENT",
            Self::Event => "EVENT",
            Self::Sale => "SALE",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProductSaleValuation {
    pub sold_at: OffsetDateTime,
    pub fx_rate_id: FxRateId,
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
    pub sale_valuation: Option<ProductSaleValuation>,
    pub state: ProductState,
    pub url: Url,
    pub images: IndexSet<ProductImage>,
    pub auction: ProductAuction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductStateChanged {
    pub old_state: ProductState,
    pub new_state: ProductState,
    pub sale_valuation: Option<ProductSaleValuation>,
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
    #[error("sold product requires a sale valuation")]
    SoldProductRequiresSaleValuation,
    #[error("sale valuation is only valid for sold or removed products")]
    SaleValuationRequiresSoldOrRemovedState,
    #[error("product geo latitude out of range")]
    GeoLatitudeOutOfRange,
    #[error("product geo longitude out of range")]
    GeoLongitudeOutOfRange,
}

#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum ProductStateTransitionError {
    #[error("a sold product cannot be reopened without an explicit sale correction")]
    SoldProductReopenRequiresExplicitOperation,
}

impl Product {
    pub fn create(input: NewProduct) -> Result<Self, RehydrateProductError> {
        let slug_id = product_slug_id(input.id, input.title.as_ref());
        let mut product = Self::rehydrate(RehydratedProductState {
            id: input.id,
            slug_id,
            shop_id: input.shop_id,
            seller_id: input.seller_id,
            shops_product_id: input.shops_product_id,
            address: input.address.clone(),
            title: input.title.clone(),
            description: input.description.clone(),
            pricing: input.pricing,
            sale_valuation: input.sale_valuation,
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
                sale_valuation: input.sale_valuation,
                state: input.state,
                url: input.url,
                images: input.images,
                auction: input.auction,
            },
        )));

        Ok(product)
    }

    #[doc(hidden)]
    #[allow(dead_code)]
    pub fn rehydrate(state: RehydratedProductState) -> Result<Self, RehydrateProductError> {
        validate_geo_address(state.address.geo)?;
        validate_sale_valuation(state.state, state.sale_valuation)?;

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
            sale_valuation: state.sale_valuation,
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

    pub fn mark_listed(&mut self) -> Result<ChangeOutcome, ProductStateTransitionError> {
        self.transition_to(ProductState::Listed)
    }

    pub fn mark_available(&mut self) -> Result<ChangeOutcome, ProductStateTransitionError> {
        self.transition_to(ProductState::Available)
    }

    pub fn mark_reserved(&mut self) -> Result<ChangeOutcome, ProductStateTransitionError> {
        self.transition_to(ProductState::Reserved)
    }

    pub fn mark_sold(
        &mut self,
        sale_valuation: ProductSaleValuation,
    ) -> Result<ChangeOutcome, ProductStateTransitionError> {
        if self.state == ProductState::Sold {
            return Ok(ChangeOutcome::Unchanged);
        }

        self.ensure_transition_allowed(ProductState::Sold)?;
        self.sale_valuation = Some(sale_valuation);
        self.record_state_change(ProductState::Sold);
        Ok(ChangeOutcome::Changed)
    }

    pub fn mark_removed(&mut self) -> Result<ChangeOutcome, ProductStateTransitionError> {
        self.transition_to(ProductState::Removed)
    }

    pub fn mark_unknown(&mut self) -> Result<ChangeOutcome, ProductStateTransitionError> {
        self.transition_to(ProductState::Unknown)
    }

    fn transition_to(
        &mut self,
        new_state: ProductState,
    ) -> Result<ChangeOutcome, ProductStateTransitionError> {
        if self.state == new_state {
            return Ok(ChangeOutcome::Unchanged);
        }

        self.ensure_transition_allowed(new_state)?;
        self.record_state_change(new_state);
        Ok(ChangeOutcome::Changed)
    }

    fn ensure_transition_allowed(
        &self,
        new_state: ProductState,
    ) -> Result<(), ProductStateTransitionError> {
        if self.sale_valuation.is_some()
            && self.state != ProductState::Sold
            && new_state != ProductState::Removed
        {
            return Err(ProductStateTransitionError::SoldProductReopenRequiresExplicitOperation);
        }
        if self.state == ProductState::Sold && new_state != ProductState::Removed {
            return Err(ProductStateTransitionError::SoldProductReopenRequiresExplicitOperation);
        }

        Ok(())
    }

    fn record_state_change(&mut self, new_state: ProductState) {
        let old_state = self.state;
        self.state = new_state;
        self.push_event(ProductDomainEventPayload::StateChanged(
            ProductStateChanged {
                old_state,
                new_state,
                sale_valuation: self.sale_valuation,
            },
        ));
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

    pub fn sale_valuation(&self) -> Option<ProductSaleValuation> {
        self.sale_valuation
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
    match title {
        Some(title) => ProductSlugId::from(title.payload.as_ref()),
        None => ProductSlugId::from(product_id.to_string()),
    }
}

fn validate_sale_valuation(
    state: ProductState,
    sale_valuation: Option<ProductSaleValuation>,
) -> Result<(), RehydrateProductError> {
    match (state, sale_valuation) {
        (ProductState::Sold, None) => Err(RehydrateProductError::SoldProductRequiresSaleValuation),
        (ProductState::Sold | ProductState::Removed, _) | (_, None) => Ok(()),
        _ => Err(RehydrateProductError::SaleValuationRequiresSoldOrRemovedState),
    }
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
    use money::Currency;
    use money::MonetaryAmount;

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
                price: Some(Price::new(MonetaryAmount::from(1_500_u64), Currency::Eur)),
                price_estimate_min: None,
                price_estimate_max: None,
            },
            sale_valuation: None,
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

    fn sale_valuation() -> ProductSaleValuation {
        ProductSaleValuation {
            sold_at: OffsetDateTime::UNIX_EPOCH,
            fx_rate_id: FxRateId::new(),
        }
    }

    #[test]
    fn should_round_trip_unique_canonical_product_price_valuation_basis_identifiers() {
        use std::collections::HashSet;
        use strum::IntoEnumIterator;

        let identifiers = ProductPriceValuationBasis::iter()
            .map(ProductPriceValuationBasis::as_str)
            .collect::<HashSet<_>>();

        assert_eq!(
            ProductPriceValuationBasis::iter().count(),
            identifiers.len()
        );
        for basis in ProductPriceValuationBasis::iter() {
            assert!(matches!(basis.as_str(), "CURRENT" | "EVENT" | "SALE"));
        }
    }

    #[test]
    fn should_reject_sold_product_creation_without_sale_valuation() {
        let mut input = new_product();
        input.state = ProductState::Sold;

        let result = Product::create(input);

        assert_eq!(
            Err(RehydrateProductError::SoldProductRequiresSaleValuation),
            result
        );
    }

    #[test]
    fn should_create_sold_product_with_sale_valuation() {
        let valuation = sale_valuation();
        let mut input = new_product();
        input.state = ProductState::Sold;
        input.sale_valuation = Some(valuation);

        let product = Product::create(input);

        assert!(matches!(
            product,
            Ok(ref product) if product.state() == ProductState::Sold
                && product.sale_valuation() == Some(valuation)
        ));
    }

    #[test]
    fn should_store_sale_valuation_in_state_change_event_when_transitioning_to_sold() {
        let valuation = sale_valuation();
        let mut product = created_product();
        product.take_pending_events();

        let result = product.mark_sold(valuation);

        assert_eq!(Ok(ChangeOutcome::Changed), result);
        assert_eq!(Some(valuation), product.sale_valuation());
        assert!(matches!(
            product.pending_events(),
            [ProductDomainEvent {
                payload: ProductDomainEventPayload::StateChanged(ProductStateChanged {
                    old_state: ProductState::Listed,
                    new_state: ProductState::Sold,
                    sale_valuation: Some(event_valuation),
                }),
                ..
            }] if *event_valuation == valuation
        ));
    }

    #[test]
    fn should_preserve_sale_valuation_when_transitioning_from_sold_to_removed() {
        let valuation = sale_valuation();
        let mut product = created_product();
        let sold = product.mark_sold(valuation);
        product.take_pending_events();

        let removed = product.mark_removed();

        assert_eq!(Ok(ChangeOutcome::Changed), sold);
        assert_eq!(Ok(ChangeOutcome::Changed), removed);
        assert_eq!(ProductState::Removed, product.state());
        assert_eq!(Some(valuation), product.sale_valuation());
    }

    #[test]
    fn should_reject_generic_reopen_of_sold_product() {
        let mut product = created_product();
        let sold = product.mark_sold(sale_valuation());
        product.take_pending_events();

        let result = product.mark_available();

        assert_eq!(Ok(ChangeOutcome::Changed), sold);
        assert_eq!(
            Err(ProductStateTransitionError::SoldProductReopenRequiresExplicitOperation),
            result
        );
        assert_eq!(ProductState::Sold, product.state());
        assert!(product.pending_events().is_empty());
    }

    #[test]
    fn should_preserve_sale_valuation_when_pricing_changes() {
        let valuation = sale_valuation();
        let mut product = created_product();
        let sold = product.mark_sold(valuation);
        product.take_pending_events();

        let outcome = product.replace_pricing(ProductPricing::default());

        assert_eq!(Ok(ChangeOutcome::Changed), sold);
        assert_eq!(ChangeOutcome::Changed, outcome);
        assert_eq!(Some(valuation), product.sale_valuation());
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
        let product = Product::rehydrate(RehydratedProductState {
            id: input.id,
            slug_id: product_slug_id(input.id, input.title.as_ref()),
            shop_id: input.shop_id,
            seller_id: input.seller_id,
            shops_product_id: input.shops_product_id,
            address: input.address,
            title: input.title,
            description: input.description,
            pricing: input.pricing,
            sale_valuation: input.sale_valuation,
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

        let outcome = product.mark_listed();

        assert_eq!(Ok(ChangeOutcome::Unchanged), outcome);
        assert!(product.pending_events().is_empty());
    }

    #[test]
    fn should_emit_event_when_state_changes() {
        let mut product = created_product();
        product.take_pending_events();

        let outcome = product.mark_available();

        assert_eq!(Ok(ChangeOutcome::Changed), outcome);
        assert!(matches!(
            product.pending_events(),
            [ProductDomainEvent {
                payload: ProductDomainEventPayload::StateChanged(ProductStateChanged {
                    old_state: ProductState::Listed,
                    new_state: ProductState::Available,
                    sale_valuation: None,
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
        let result = Product::rehydrate(RehydratedProductState {
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
            sale_valuation: input.sale_valuation,
            state: input.state,
            lifecycle: ProductLifecycle::Active,
            url: input.url,
            images: input.images,
            auction: input.auction,
        });

        assert_eq!(Err(RehydrateProductError::GeoLatitudeOutOfRange), result);
    }

    #[test]
    fn should_reject_invalid_longitude_when_rehydrating() {
        let input = new_product();
        let result = Product::rehydrate(RehydratedProductState {
            id: input.id,
            slug_id: product_slug_id(input.id, input.title.as_ref()),
            shop_id: input.shop_id,
            seller_id: input.seller_id,
            shops_product_id: input.shops_product_id,
            address: ProductAddress {
                structured: None,
                geo: Some(GeoAddress {
                    lat: 0.0,
                    lon: 181.0,
                }),
            },
            title: input.title,
            description: input.description,
            pricing: input.pricing,
            sale_valuation: input.sale_valuation,
            state: input.state,
            lifecycle: ProductLifecycle::Active,
            url: input.url,
            images: input.images,
            auction: input.auction,
        });

        assert_eq!(Err(RehydrateProductError::GeoLongitudeOutOfRange), result);
    }

    #[test]
    fn should_accept_geo_boundaries_when_rehydrating() {
        let input = new_product();
        let result = Product::rehydrate(RehydratedProductState {
            id: input.id,
            slug_id: product_slug_id(input.id, input.title.as_ref()),
            shop_id: input.shop_id,
            seller_id: input.seller_id,
            shops_product_id: input.shops_product_id,
            address: ProductAddress {
                structured: None,
                geo: Some(GeoAddress {
                    lat: -90.0,
                    lon: 180.0,
                }),
            },
            title: input.title,
            description: input.description,
            pricing: input.pricing,
            sale_valuation: input.sale_valuation,
            state: input.state,
            lifecycle: ProductLifecycle::Active,
            url: input.url,
            images: input.images,
            auction: input.auction,
        });

        assert!(result.is_ok());
    }

    #[test]
    fn should_use_title_for_slug_when_product_created() {
        let product = created_product();

        assert!(product.slug_id().as_ref().starts_with("bronze-vase"));
    }

    #[test]
    fn should_use_product_id_for_slug_when_title_missing() {
        let mut input = new_product();
        input.title = None;
        let product_id = input.id;

        let product = Product::create(input);

        assert!(
            matches!(product, Ok(ref product) if product.slug_id().as_ref().starts_with(&format!("{product_id}-")))
        );
    }

    #[test]
    fn should_emit_event_when_address_changes() {
        let mut product = created_product();
        product.take_pending_events();
        let address = ProductAddress {
            structured: None,
            geo: Some(GeoAddress {
                lat: 10.0,
                lon: 20.0,
            }),
        };

        let outcome = product.replace_address(address.clone());

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert!(matches!(
            product.pending_events(),
            [ProductDomainEvent {
                payload: ProductDomainEventPayload::AddressChanged(ProductAddressChanged { address: event_address }),
                ..
            }] if *event_address == address
        ));
    }

    #[test]
    fn should_not_emit_event_when_address_unchanged() {
        let mut product = created_product();
        product.take_pending_events();
        let address = product.address();

        let outcome = product.replace_address(address);

        assert_eq!(ChangeOutcome::Unchanged, outcome);
        assert!(product.pending_events().is_empty());
    }

    #[test]
    fn should_emit_event_when_pricing_changes() {
        let mut product = created_product();
        product.take_pending_events();
        let pricing = ProductPricing::default();

        let outcome = product.replace_pricing(pricing);

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert!(matches!(
            product.pending_events(),
            [ProductDomainEvent {
                payload: ProductDomainEventPayload::PriceChanged(_),
                ..
            }]
        ));
    }

    #[test]
    fn should_not_emit_event_when_pricing_unchanged() {
        let mut product = created_product();
        product.take_pending_events();

        let outcome = product.replace_pricing(product.pricing());

        assert_eq!(ChangeOutcome::Unchanged, outcome);
        assert!(product.pending_events().is_empty());
    }

    #[test]
    fn should_emit_event_when_url_changes() {
        let mut product = created_product();
        product.take_pending_events();
        let new_url = Url::parse("https://shop.example/products/2")
            .unwrap_or_else(|error| panic!("invalid URL: {error}"));

        let outcome = product.change_url(new_url.clone());

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert!(matches!(
            product.pending_events(),
            [ProductDomainEvent {
                payload: ProductDomainEventPayload::UrlChanged(ProductUrlChanged { new_url: event_url, .. }),
                ..
            }] if *event_url == new_url
        ));
    }

    #[test]
    fn should_not_emit_event_when_url_unchanged() {
        let mut product = created_product();
        product.take_pending_events();
        let url = product.url().clone();

        let outcome = product.change_url(url);

        assert_eq!(ChangeOutcome::Unchanged, outcome);
        assert!(product.pending_events().is_empty());
    }

    #[test]
    fn should_emit_event_when_images_change() {
        let mut product = created_product();
        product.take_pending_events();
        let mut images = IndexSet::new();
        images.insert(ProductImage {
            url: Url::parse("https://shop.example/image.jpg")
                .unwrap_or_else(|error| panic!("invalid URL: {error}")),
            prohibited_content: crate::prohibited_content::ProhibitedContent::None,
        });

        let outcome = product.replace_images(images.clone());

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert!(matches!(
            product.pending_events(),
            [ProductDomainEvent {
                payload: ProductDomainEventPayload::ImagesChanged(payload),
                ..
            }] if payload.images == images
        ));
    }

    #[test]
    fn should_not_emit_event_when_images_unchanged() {
        let mut product = created_product();
        product.take_pending_events();
        let images = product.images().clone();

        let outcome = product.replace_images(images);

        assert_eq!(ChangeOutcome::Unchanged, outcome);
        assert!(product.pending_events().is_empty());
    }

    #[test]
    fn should_emit_event_when_auction_changes() {
        let mut product = created_product();
        product.take_pending_events();
        let auction = ProductAuction {
            start: Some(OffsetDateTime::UNIX_EPOCH),
            end: None,
        };

        let outcome = product.replace_auction(auction);

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert!(matches!(
            product.pending_events(),
            [ProductDomainEvent {
                payload: ProductDomainEventPayload::AuctionChanged(_),
                ..
            }]
        ));
    }

    #[test]
    fn should_not_emit_event_when_auction_unchanged() {
        let mut product = created_product();
        product.take_pending_events();

        let outcome = product.replace_auction(product.auction());

        assert_eq!(ChangeOutcome::Unchanged, outcome);
        assert!(product.pending_events().is_empty());
    }

    #[test]
    fn should_emit_event_when_deleted() {
        let mut product = created_product();
        product.take_pending_events();

        let outcome = product.delete();

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert_eq!(ProductLifecycle::Deleted, product.lifecycle());
        assert!(matches!(
            product.pending_events(),
            [ProductDomainEvent {
                payload: ProductDomainEventPayload::Deleted(_),
                ..
            }]
        ));
    }

    #[test]
    fn should_not_emit_event_when_deleted_twice() {
        let mut product = created_product();
        product.delete();
        product.take_pending_events();

        let outcome = product.delete();

        assert_eq!(ChangeOutcome::Unchanged, outcome);
        assert!(product.pending_events().is_empty());
    }

    #[rstest::rstest]
    #[case(ProductDomainEventPayload::Created(Box::new(ProductCreated {
        title: None,
        description: None,
        address: ProductAddress::default(),
        pricing: ProductPricing::default(),
        sale_valuation: None,
        state: ProductState::Listed,
        url: test_url(),
        images: IndexSet::new(),
        auction: ProductAuction::default(),
    })), "PRODUCT_CREATED")]
    #[case(ProductDomainEventPayload::StateChanged(ProductStateChanged { old_state: ProductState::Listed, new_state: ProductState::Available, sale_valuation: None }), "PRODUCT_STATE_CHANGED")]
    #[case(ProductDomainEventPayload::AddressChanged(ProductAddressChanged { address: ProductAddress::default() }), "PRODUCT_ADDRESS_CHANGED")]
    #[case(ProductDomainEventPayload::PriceChanged(ProductPriceChanged { old_pricing: ProductPricing::default(), new_pricing: ProductPricing::default() }), "PRODUCT_PRICE_CHANGED")]
    #[case(ProductDomainEventPayload::UrlChanged(ProductUrlChanged { old_url: test_url(), new_url: test_url() }), "PRODUCT_URL_CHANGED")]
    #[case(ProductDomainEventPayload::ImagesChanged(Box::new(ProductImagesChanged { images: IndexSet::new() })), "PRODUCT_IMAGES_CHANGED")]
    #[case(ProductDomainEventPayload::AuctionChanged(ProductAuctionChanged { auction: ProductAuction::default() }), "PRODUCT_AUCTION_CHANGED")]
    #[case(ProductDomainEventPayload::Deleted(ProductDeleted { old_lifecycle: ProductLifecycle::Active, new_lifecycle: ProductLifecycle::Deleted }), "PRODUCT_DELETED")]
    fn should_return_event_type_for_each_product_domain_event(
        #[case] payload: ProductDomainEventPayload,
        #[case] expected: &'static str,
    ) {
        assert_eq!(expected, payload.event_type());
    }
}
