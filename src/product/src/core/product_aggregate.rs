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
    native_title: Localized<Language, Title>,
    native_description: Option<Localized<Language, Description>>,
    pricing: ProductPricing,
    state: ProductState,
    lifecycle: ProductLifecycle,
    url: Url,
    images: IndexSet<ProductImage>,
    embedding: Option<Vec<f32>>,
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
    pub native_title: Localized<Language, Title>,
    pub native_description: Option<Localized<Language, Description>>,
    pub pricing: ProductPricing,
    pub state: ProductState,
    pub url: Url,
    pub images: IndexSet<ProductImage>,
    pub embedding: Option<Vec<f32>>,
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
    pub native_title: Localized<Language, Title>,
    pub native_description: Option<Localized<Language, Description>>,
    pub pricing: ProductPricing,
    pub state: ProductState,
    pub lifecycle: ProductLifecycle,
    pub url: Url,
    pub images: IndexSet<ProductImage>,
    pub embedding: Option<Vec<f32>>,
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
    Embedded(ProductEmbedded),
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
            ProductDomainEventPayload::Embedded(_) => "PRODUCT_EMBEDDED",
            ProductDomainEventPayload::AuctionChanged(_) => "PRODUCT_AUCTION_CHANGED",
            ProductDomainEventPayload::Deleted(_) => "PRODUCT_DELETED",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductCreated {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub address: ProductAddress,
    pub native_title: Localized<Language, Title>,
    pub native_description: Option<Localized<Language, Description>>,
    pub pricing: ProductPricing,
    pub state: ProductState,
    pub url: Url,
    pub images: IndexSet<ProductImage>,
    pub embedding: Option<Vec<f32>>,
    pub auction: ProductAuction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductStateChanged {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub old_state: ProductState,
    pub new_state: ProductState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductAddressChanged {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub address: ProductAddress,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductPriceChanged {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub old_pricing: ProductPricing,
    pub new_pricing: ProductPricing,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductUrlChanged {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub old_url: Url,
    pub new_url: Url,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductImagesChanged {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub images: IndexSet<ProductImage>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductEmbedded {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductAuctionChanged {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub auction: ProductAuction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductDeleted {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
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
        let mut product = Self::rehydrate(ProductStateSnapshot {
            id: input.id,
            slug_id: ProductSlugId::from(input.native_title.payload.as_ref()),
            shop_id: input.shop_id,
            seller_id: input.seller_id,
            shops_product_id: input.shops_product_id.clone(),
            address: input.address.clone(),
            native_title: input.native_title.clone(),
            native_description: input.native_description.clone(),
            pricing: input.pricing,
            state: input.state,
            lifecycle: ProductLifecycle::Active,
            url: input.url.clone(),
            images: input.images.clone(),
            embedding: input.embedding.clone(),
            auction: input.auction,
        })?;

        product.push_event(ProductDomainEventPayload::Created(Box::new(
            ProductCreated {
                shop_id: input.shop_id,
                seller_id: input.seller_id,
                shops_product_id: input.shops_product_id,
                address: input.address,
                native_title: input.native_title,
                native_description: input.native_description,
                pricing: input.pricing,
                state: input.state,
                url: input.url,
                images: input.images,
                embedding: input.embedding,
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
            native_title: state.native_title,
            native_description: state.native_description,
            pricing: state.pricing,
            state: state.state,
            lifecycle: state.lifecycle,
            url: state.url,
            images: state.images,
            embedding: state.embedding,
            auction: state.auction,
            pending_events: Vec::new(),
        })
    }

    pub fn replace_address(&mut self, address: ProductAddress) -> ChangeOutcome {
        if replace_if_changed(&mut self.address, address.clone()) == ChangeOutcome::Unchanged {
            return ChangeOutcome::Unchanged;
        }

        self.push_event(ProductDomainEventPayload::AddressChanged(
            ProductAddressChanged {
                shop_id: self.shop_id,
                shops_product_id: self.shops_product_id.clone(),
                address,
            },
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
                shop_id: self.shop_id,
                shops_product_id: self.shops_product_id.clone(),
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
                shop_id: self.shop_id,
                shops_product_id: self.shops_product_id.clone(),
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
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id.clone(),
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
            ProductImagesChanged {
                shop_id: self.shop_id,
                shops_product_id: self.shops_product_id.clone(),
                images,
            },
        )));
        ChangeOutcome::Changed
    }

    pub fn replace_embedding(&mut self, embedding: Option<Vec<f32>>) -> ChangeOutcome {
        if self.embedding == embedding {
            return ChangeOutcome::Unchanged;
        }

        self.embedding = embedding.clone();
        self.push_event(ProductDomainEventPayload::Embedded(ProductEmbedded {
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id.clone(),
            embedding,
        }));
        ChangeOutcome::Changed
    }

    pub fn replace_auction(&mut self, auction: ProductAuction) -> ChangeOutcome {
        if self.auction == auction {
            return ChangeOutcome::Unchanged;
        }

        self.auction = auction;
        self.push_event(ProductDomainEventPayload::AuctionChanged(
            ProductAuctionChanged {
                shop_id: self.shop_id,
                shops_product_id: self.shops_product_id.clone(),
                auction,
            },
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
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id.clone(),
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

    pub fn native_title(&self) -> &Localized<Language, Title> {
        &self.native_title
    }

    pub fn native_description(&self) -> Option<&Localized<Language, Description>> {
        self.native_description.as_ref()
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

    pub fn embedding(&self) -> Option<&Vec<f32>> {
        self.embedding.as_ref()
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
    use common::localized::Localized;
    use common::price::domain::MonetaryAmount;
    use url::Url;

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
            native_title: Localized::new(Language::En, Title::from("Bronze vase")),
            native_description: None,
            pricing: ProductPricing {
                native_price: Some(Price::new(MonetaryAmount::from(1_500_u64), Currency::Eur)),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                fx_rate_id: Some(FxRateId::new()),
            },
            state: ProductState::Listed,
            url: test_url(),
            images: IndexSet::new(),
            embedding: None,
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
    fn should_rehydrate_without_pending_events() {
        let input = new_product();
        let product = Product::rehydrate(ProductStateSnapshot {
            id: input.id,
            slug_id: ProductSlugId::from(input.native_title.payload.as_ref()),
            shop_id: input.shop_id,
            seller_id: input.seller_id,
            shops_product_id: input.shops_product_id,
            address: input.address,
            native_title: input.native_title,
            native_description: input.native_description,
            pricing: input.pricing,
            state: input.state,
            lifecycle: ProductLifecycle::Active,
            url: input.url,
            images: input.images,
            embedding: input.embedding,
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
                    ..
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
    fn should_emit_event_when_embedding_clears() {
        let mut product = created_product();
        product.take_pending_events();
        product.replace_embedding(Some(vec![0.1]));
        product.take_pending_events();

        let outcome = product.replace_embedding(None);

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert!(matches!(
            product.pending_events(),
            [ProductDomainEvent {
                payload: ProductDomainEventPayload::Embedded(ProductEmbedded {
                    embedding: None,
                    ..
                }),
                ..
            }]
        ));
    }

    #[test]
    fn should_reject_invalid_geo_when_rehydrating() {
        let input = new_product();
        let result = Product::rehydrate(ProductStateSnapshot {
            id: input.id,
            slug_id: ProductSlugId::from(input.native_title.payload.as_ref()),
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
            native_title: input.native_title,
            native_description: input.native_description,
            pricing: input.pricing,
            state: input.state,
            lifecycle: ProductLifecycle::Active,
            url: input.url,
            images: input.images,
            embedding: input.embedding,
            auction: input.auction,
        });

        assert_eq!(Err(RehydrateProductError::GeoLatitudeOutOfRange), result);
    }
}
