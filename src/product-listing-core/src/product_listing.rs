use crate::description::Description;
use crate::listing_availability::ListingAvailability;
use crate::listing_lifecycle::ListingLifecycle;
use crate::product_listing_event::{
    ListingSaleObservationChange, ProductListingChanged, ProductListingDiscovered,
    ProductListingEventPayload, ProductListingLifecycleChange,
};
use crate::product_listing_id::ProductListingId;
use crate::product_listing_image::ProductListingImage;
use crate::product_listing_slug_id::ProductListingSlugId;
use crate::source_listing_id::SourceListingId;
use crate::title::Title;
use domain_primitives::change_outcome::ChangeOutcome;
use fxrate_core::FxRateId;
use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use money::Price;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListing {
    id: ProductListingId,
    title_slug_id: ProductListingSlugId,
    listing_source_id: ListingSourceId,
    source_listing_id: SourceListingId,
    title: Option<Localized<Language, Title>>,
    description: Option<Localized<Language, Description>>,
    pricing: ProductListingPricing,
    sale_observation: Option<ListingSaleObservation>,
    availability: Option<ListingAvailability>,
    lifecycle: ListingLifecycle,
    url: Url,
    images: IndexSet<ProductListingImage>,
    auction: ProductListingAuction,
    pending_event_payload: Option<ProductListingEventPayload>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewProductListing {
    pub id: ProductListingId,
    pub title_slug_id: ProductListingSlugId,
    pub listing_source_id: ListingSourceId,
    pub source_listing_id: SourceListingId,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub pricing: ProductListingPricing,
    pub availability: Option<ListingAvailability>,
    pub url: Url,
    pub images: IndexSet<ProductListingImage>,
    pub auction: ProductListingAuction,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedProductListingState {
    pub id: ProductListingId,
    pub title_slug_id: ProductListingSlugId,
    pub listing_source_id: ListingSourceId,
    pub source_listing_id: SourceListingId,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub pricing: ProductListingPricing,
    pub sale_observation: Option<ListingSaleObservation>,
    pub availability: Option<ListingAvailability>,
    pub lifecycle: ListingLifecycle,
    pub url: Url,
    pub images: IndexSet<ProductListingImage>,
    pub auction: ProductListingAuction,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ProductListingPricing {
    pub price: Option<Price>,
    pub price_estimate_min: Option<Price>,
    pub price_estimate_max: Option<Price>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::EnumIter)]
pub enum ProductListingPriceValuationBasis {
    Current,
    Event,
    SaleObservation,
}

impl ProductListingPriceValuationBasis {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Current => "CURRENT",
            Self::Event => "EVENT",
            Self::SaleObservation => "SALE_OBSERVATION",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingSaleObservation {
    observed_at: OffsetDateTime,
    fx_rate_id: FxRateId,
}

impl ListingSaleObservation {
    pub const fn new(observed_at: OffsetDateTime, fx_rate_id: FxRateId) -> Self {
        Self {
            observed_at,
            fx_rate_id,
        }
    }

    pub const fn observed_at(self) -> OffsetDateTime {
        self.observed_at
    }

    pub const fn fx_rate_id(self) -> FxRateId {
        self.fx_rate_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ProductListingAuction {
    pub start: Option<OffsetDateTime>,
    pub end: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RehydrateProductListingError {
    #[error("product listing title slug is invalid")]
    InvalidTitleSlugId,
    #[error("withdrawn listing has availability")]
    WithdrawnListingHasAvailability,
    #[error("product listing auction start is after its end")]
    AuctionStartAfterEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProductListingInvariantError {
    AuctionStartAfterEnd,
}

impl From<ProductListingInvariantError> for RehydrateProductListingError {
    fn from(error: ProductListingInvariantError) -> Self {
        match error {
            ProductListingInvariantError::AuctionStartAfterEnd => Self::AuctionStartAfterEnd,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ChangeListingAvailabilityError {
    #[error("listing is withdrawn")]
    ListingWithdrawn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ChangeProductListingError {
    #[error("listing is withdrawn")]
    ListingWithdrawn,
    #[error("product listing auction start is after its end")]
    AuctionStartAfterEnd,
}

impl From<ProductListingInvariantError> for ChangeProductListingError {
    fn from(error: ProductListingInvariantError) -> Self {
        match error {
            ProductListingInvariantError::AuctionStartAfterEnd => Self::AuctionStartAfterEnd,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RecordListingSaleObservationError {
    #[error("a different sale observation already exists")]
    ConflictingExistingObservation,
}

impl ProductListing {
    /// Creates a listing from explicit, deterministic identity values.
    pub fn create(input: NewProductListing) -> Result<Self, RehydrateProductListingError> {
        let mut listing = Self::rehydrate(RehydratedProductListingState {
            id: input.id,
            title_slug_id: input.title_slug_id,
            listing_source_id: input.listing_source_id,
            source_listing_id: input.source_listing_id,
            title: input.title,
            description: input.description,
            pricing: input.pricing,
            sale_observation: None,
            availability: input.availability,
            lifecycle: ListingLifecycle::Active,
            url: input.url,
            images: input.images,
            auction: input.auction,
        })?;
        listing.pending_event_payload = Some(ProductListingEventPayload::Discovered(
            listing.discovered_event(),
        ));
        Ok(listing)
    }

    #[doc(hidden)]
    pub fn rehydrate(
        state: RehydratedProductListingState,
    ) -> Result<Self, RehydrateProductListingError> {
        validate_auction(state.auction).map_err(RehydrateProductListingError::from)?;
        if state.lifecycle == ListingLifecycle::Withdrawn && state.availability.is_some() {
            return Err(RehydrateProductListingError::WithdrawnListingHasAvailability);
        }
        Ok(Self {
            id: state.id,
            title_slug_id: state.title_slug_id,
            listing_source_id: state.listing_source_id,
            source_listing_id: state.source_listing_id,
            title: state.title,
            description: state.description,
            pricing: state.pricing,
            sale_observation: state.sale_observation,
            availability: state.availability,
            lifecycle: state.lifecycle,
            url: state.url,
            images: state.images,
            auction: state.auction,
            pending_event_payload: None,
        })
    }

    pub fn set_availability(
        &mut self,
        availability: ListingAvailability,
    ) -> Result<ChangeOutcome, ChangeListingAvailabilityError> {
        self.ensure_active_availability()?;
        if self.availability == Some(availability) {
            return Ok(ChangeOutcome::Unchanged);
        }
        self.change_availability(Some(availability));
        Ok(ChangeOutcome::Changed)
    }

    pub fn clear_availability(&mut self) -> Result<ChangeOutcome, ChangeListingAvailabilityError> {
        self.ensure_active_availability()?;
        if self.availability.is_none() {
            return Ok(ChangeOutcome::Unchanged);
        }
        self.change_availability(None);
        Ok(ChangeOutcome::Changed)
    }

    pub fn withdraw(&mut self) -> ChangeOutcome {
        if self.lifecycle == ListingLifecycle::Withdrawn {
            return ChangeOutcome::Unchanged;
        }
        let previous_availability = self.availability;
        self.lifecycle = ListingLifecycle::Withdrawn;
        self.availability = None;
        self.coalesce_pending_change(|changed| {
            changed.change_availability(previous_availability, None);
            changed.change_lifecycle(ProductListingLifecycleChange::Withdrawn {
                previous_availability,
            });
        });
        ChangeOutcome::Changed
    }

    pub fn restore(&mut self) -> ChangeOutcome {
        if self.lifecycle == ListingLifecycle::Active {
            return ChangeOutcome::Unchanged;
        }
        self.lifecycle = ListingLifecycle::Active;
        self.availability = None;
        self.coalesce_pending_change(|changed| {
            changed.change_lifecycle(ProductListingLifecycleChange::Restored);
        });
        ChangeOutcome::Changed
    }

    pub fn record_sale_observation(
        &mut self,
        observation: ListingSaleObservation,
    ) -> Result<ChangeOutcome, RecordListingSaleObservationError> {
        match self.sale_observation {
            None => {
                self.sale_observation = Some(observation);
                self.coalesce_pending_change(|changed| {
                    changed.change_sale_observation(ListingSaleObservationChange::Observed(
                        observation,
                    ));
                });
                Ok(ChangeOutcome::Changed)
            }
            Some(existing) if existing == observation => Ok(ChangeOutcome::Unchanged),
            Some(_) => Err(RecordListingSaleObservationError::ConflictingExistingObservation),
        }
    }

    pub fn retract_sale_observation(&mut self) -> ChangeOutcome {
        let Some(observation) = self.sale_observation.take() else {
            return ChangeOutcome::Unchanged;
        };
        self.coalesce_pending_change(|changed| {
            changed.change_sale_observation(ListingSaleObservationChange::Retracted(observation));
        });
        ChangeOutcome::Changed
    }

    pub fn replace_pricing(
        &mut self,
        pricing: ProductListingPricing,
    ) -> Result<ChangeOutcome, ChangeProductListingError> {
        self.ensure_active_mutation()?;
        if self.pricing == pricing {
            return Ok(ChangeOutcome::Unchanged);
        }
        let previous = self.pricing;
        self.pricing = pricing;
        self.coalesce_pending_change(|changed| {
            if previous.price != pricing.price {
                changed.change_price(previous.price, pricing.price);
            }
            if previous.price_estimate_min != pricing.price_estimate_min {
                changed.change_price_estimate_min(
                    previous.price_estimate_min,
                    pricing.price_estimate_min,
                );
            }
            if previous.price_estimate_max != pricing.price_estimate_max {
                changed.change_price_estimate_max(
                    previous.price_estimate_max,
                    pricing.price_estimate_max,
                );
            }
        });
        Ok(ChangeOutcome::Changed)
    }

    pub fn set_price(&mut self, price: Price) -> Result<ChangeOutcome, ChangeProductListingError> {
        let mut pricing = self.pricing;
        pricing.price = Some(price);
        self.replace_pricing(pricing)
    }

    pub fn clear_price(&mut self) -> Result<ChangeOutcome, ChangeProductListingError> {
        let mut pricing = self.pricing;
        pricing.price = None;
        self.replace_pricing(pricing)
    }

    pub fn change_url(&mut self, url: Url) -> Result<ChangeOutcome, ChangeProductListingError> {
        self.ensure_active_mutation()?;
        if self.url == url {
            return Ok(ChangeOutcome::Unchanged);
        }
        let previous = self.url.clone();
        self.url = url;
        let current = self.url.clone();
        self.coalesce_pending_change(|changed| changed.change_url(previous, current));
        Ok(ChangeOutcome::Changed)
    }

    pub fn replace_images(
        &mut self,
        images: IndexSet<ProductListingImage>,
    ) -> Result<ChangeOutcome, ChangeProductListingError> {
        self.ensure_active_mutation()?;
        if self.images == images {
            return Ok(ChangeOutcome::Unchanged);
        }
        let previous_count = self.images.len();
        self.images = images;
        let current_count = self.images.len();
        self.coalesce_pending_change(|changed| {
            changed.change_image_count(previous_count, current_count);
        });
        Ok(ChangeOutcome::Changed)
    }

    pub fn replace_auction(
        &mut self,
        auction: ProductListingAuction,
    ) -> Result<ChangeOutcome, ChangeProductListingError> {
        self.ensure_active_mutation()?;
        validate_auction(auction).map_err(ChangeProductListingError::from)?;
        if self.auction == auction {
            return Ok(ChangeOutcome::Unchanged);
        }
        let previous = self.auction;
        self.auction = auction;
        self.coalesce_pending_change(|changed| changed.change_auction(previous, auction));
        Ok(ChangeOutcome::Changed)
    }

    pub fn take_pending_event_payload(&mut self) -> Option<ProductListingEventPayload> {
        self.pending_event_payload.take()
    }

    pub fn id(&self) -> ProductListingId {
        self.id
    }
    pub fn title_slug_id(&self) -> &ProductListingSlugId {
        &self.title_slug_id
    }
    pub fn listing_source_id(&self) -> ListingSourceId {
        self.listing_source_id
    }
    pub fn source_listing_id(&self) -> &SourceListingId {
        &self.source_listing_id
    }
    pub fn title(&self) -> Option<&Localized<Language, Title>> {
        self.title.as_ref()
    }
    pub fn description(&self) -> Option<&Localized<Language, Description>> {
        self.description.as_ref()
    }
    pub fn pricing(&self) -> ProductListingPricing {
        self.pricing
    }
    pub fn sale_observation(&self) -> Option<ListingSaleObservation> {
        self.sale_observation
    }
    pub fn availability(&self) -> Option<ListingAvailability> {
        self.availability
    }
    pub fn lifecycle(&self) -> ListingLifecycle {
        self.lifecycle
    }
    pub fn url(&self) -> &Url {
        &self.url
    }
    pub fn images(&self) -> &IndexSet<ProductListingImage> {
        &self.images
    }
    pub fn auction(&self) -> ProductListingAuction {
        self.auction
    }

    fn ensure_active_availability(&self) -> Result<(), ChangeListingAvailabilityError> {
        if self.lifecycle == ListingLifecycle::Withdrawn {
            Err(ChangeListingAvailabilityError::ListingWithdrawn)
        } else {
            Ok(())
        }
    }

    fn ensure_active_mutation(&self) -> Result<(), ChangeProductListingError> {
        if self.lifecycle == ListingLifecycle::Withdrawn {
            Err(ChangeProductListingError::ListingWithdrawn)
        } else {
            Ok(())
        }
    }

    fn change_availability(&mut self, current: Option<ListingAvailability>) {
        let previous = self.availability;
        if previous == current {
            return;
        }
        self.availability = current;
        self.coalesce_pending_change(|changed| changed.change_availability(previous, current));
    }

    fn discovered_event(&self) -> ProductListingDiscovered {
        ProductListingDiscovered::new(
            self.listing_source_id,
            self.source_listing_id.clone(),
            self.title.clone(),
            self.description.clone(),
            self.pricing,
            self.availability,
            self.url.clone(),
            self.images.len(),
            self.auction,
        )
    }

    fn coalesce_pending_change(&mut self, change: impl FnOnce(&mut ProductListingChanged)) {
        if matches!(
            self.pending_event_payload,
            Some(ProductListingEventPayload::Discovered(_))
        ) {
            self.pending_event_payload = Some(ProductListingEventPayload::Discovered(
                self.discovered_event(),
            ));
            return;
        }

        let mut changed = match self.pending_event_payload.take() {
            Some(ProductListingEventPayload::Changed(changed)) => changed,
            None => ProductListingChanged::empty(),
            Some(ProductListingEventPayload::Discovered(_)) => return,
        };
        change(&mut changed);
        self.pending_event_payload =
            (!changed.is_empty()).then_some(ProductListingEventPayload::Changed(changed));
    }
}

fn validate_auction(auction: ProductListingAuction) -> Result<(), ProductListingInvariantError> {
    if let (Some(start), Some(end)) = (auction.start, auction.end)
        && start > end
    {
        return Err(ProductListingInvariantError::AuctionStartAfterEnd);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::product_listing_event::{ProductListingEventType, ValueChange};

    fn input() -> NewProductListing {
        NewProductListing {
            id: ProductListingId::new(),
            title_slug_id: ProductListingSlugId::raw("listing-a1b2c3")
                .unwrap_or_else(|error| panic!("valid test title slug: {error}")),
            listing_source_id: ListingSourceId::new(),
            source_listing_id: SourceListingId::try_from("source-listing-id")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            title: None,
            description: None,
            pricing: ProductListingPricing::default(),
            availability: None,
            url: Url::parse("https://shop.example/listing")
                .unwrap_or_else(|error| panic!("URL: {error}")),
            images: IndexSet::new(),
            auction: ProductListingAuction::default(),
        }
    }

    fn rehydrated() -> ProductListing {
        let source = input();
        ProductListing::rehydrate(RehydratedProductListingState {
            id: source.id,
            title_slug_id: source.title_slug_id,
            listing_source_id: source.listing_source_id,
            source_listing_id: source.source_listing_id,
            title: source.title,
            description: source.description,
            pricing: source.pricing,
            sale_observation: None,
            availability: source.availability,
            lifecycle: ListingLifecycle::Active,
            url: source.url,
            images: source.images,
            auction: source.auction,
        })
        .unwrap_or_else(|error| panic!("rehydrate: {error}"))
    }

    fn eur_price(amount: u64) -> Price {
        Price::new(money::MonetaryAmount::from(amount), money::Currency::Eur)
    }

    fn observation() -> ListingSaleObservation {
        ListingSaleObservation::new(OffsetDateTime::UNIX_EPOCH, FxRateId::new())
    }

    #[test]
    fn should_emit_discovered_with_semantic_facts_and_image_count() {
        let mut source = input();
        source.pricing.price = Some(eur_price(100));
        source.availability = Some(ListingAvailability::InStock);
        source.images.insert(ProductListingImage::new(
            Url::parse("https://shop.example/a.jpg").unwrap_or_else(|error| panic!("URL: {error}")),
        ));
        let source_listing_id = source.source_listing_id.clone();

        let mut listing =
            ProductListing::create(source).unwrap_or_else(|error| panic!("create: {error}"));

        let Some(ProductListingEventPayload::Discovered(discovered)) =
            listing.take_pending_event_payload()
        else {
            panic!("expected discovered payload");
        };
        assert_eq!(
            ProductListingEventType::Discovered,
            ProductListingEventPayload::Discovered(discovered.clone()).event_type()
        );
        assert_eq!(&source_listing_id, discovered.source_listing_id());
        assert_eq!(Some(eur_price(100)), discovered.pricing().price);
        assert_eq!(
            Some(ListingAvailability::InStock),
            discovered.availability()
        );
        assert_eq!(1, discovered.image_count());
    }

    #[test]
    fn should_update_discovered_to_final_creation_side_state_without_second_event() {
        let mut listing =
            ProductListing::create(input()).unwrap_or_else(|error| panic!("create: {error}"));
        let replacement_url =
            Url::parse("https://shop.example/final").unwrap_or_else(|error| panic!("URL: {error}"));

        listing
            .set_price(eur_price(150))
            .unwrap_or_else(|error| panic!("set price: {error}"));
        listing
            .set_availability(ListingAvailability::InStock)
            .unwrap_or_else(|error| panic!("set availability: {error}"));
        listing
            .change_url(replacement_url.clone())
            .unwrap_or_else(|error| panic!("change URL: {error}"));

        let Some(ProductListingEventPayload::Discovered(discovered)) =
            listing.take_pending_event_payload()
        else {
            panic!("expected one discovered payload");
        };
        assert_eq!(Some(eur_price(150)), discovered.pricing().price);
        assert_eq!(
            Some(ListingAvailability::InStock),
            discovered.availability()
        );
        assert_eq!(&replacement_url, discovered.url());

        assert_eq!(None, listing.take_pending_event_payload());
    }

    #[test]
    fn should_coalesce_split_price_changes_and_remove_net_zero_dimensions() {
        let mut listing = rehydrated();
        listing
            .replace_pricing(ProductListingPricing {
                price: Some(eur_price(100)),
                price_estimate_min: Some(eur_price(80)),
                price_estimate_max: Some(eur_price(120)),
            })
            .unwrap_or_else(|error| panic!("replace pricing: {error}"));
        listing
            .replace_pricing(ProductListingPricing {
                price: Some(eur_price(150)),
                price_estimate_min: None,
                price_estimate_max: Some(eur_price(120)),
            })
            .unwrap_or_else(|error| panic!("replace pricing: {error}"));

        let Some(ProductListingEventPayload::Changed(changed)) =
            listing.take_pending_event_payload()
        else {
            panic!("expected changed payload");
        };
        assert_eq!(Some(&None), changed.price().map(ValueChange::previous));
        assert_eq!(
            Some(&Some(eur_price(150))),
            changed.price().map(ValueChange::current)
        );
        assert_eq!(None, changed.price_estimate_min());
        assert_eq!(
            Some(&None),
            changed.price_estimate_max().map(ValueChange::previous)
        );
        assert_eq!(
            Some(&Some(eur_price(120))),
            changed.price_estimate_max().map(ValueChange::current)
        );
    }

    #[test]
    fn should_coalesce_availability_url_and_auction_with_first_previous_and_final_current() {
        let mut listing = rehydrated();
        let first_url = listing.url().clone();
        let final_url =
            Url::parse("https://shop.example/final").unwrap_or_else(|error| panic!("URL: {error}"));
        let first_auction = ProductListingAuction::default();
        let final_auction = ProductListingAuction {
            start: Some(OffsetDateTime::UNIX_EPOCH),
            end: Some(OffsetDateTime::UNIX_EPOCH + time::Duration::hours(1)),
        };

        listing
            .set_availability(ListingAvailability::Available)
            .unwrap_or_else(|error| panic!("set availability: {error}"));
        listing
            .change_url(final_url.clone())
            .unwrap_or_else(|error| panic!("change URL: {error}"));
        listing
            .replace_auction(final_auction)
            .unwrap_or_else(|error| panic!("replace auction: {error}"));

        let Some(ProductListingEventPayload::Changed(changed)) =
            listing.take_pending_event_payload()
        else {
            panic!("expected changed payload");
        };
        assert_eq!(
            Some(&None),
            changed.availability().map(ValueChange::previous)
        );
        assert_eq!(
            Some(&Some(ListingAvailability::Available)),
            changed.availability().map(ValueChange::current)
        );
        assert_eq!(Some(&first_url), changed.url().map(ValueChange::previous));
        assert_eq!(Some(&final_url), changed.url().map(ValueChange::current));
        assert_eq!(
            Some(&first_auction),
            changed.auction().map(ValueChange::previous)
        );
        assert_eq!(
            Some(&final_auction),
            changed.auction().map(ValueChange::current)
        );
    }

    #[test]
    fn should_keep_image_change_when_replacement_preserves_count() {
        let mut source = input();
        source.images.insert(ProductListingImage::new(
            Url::parse("https://shop.example/old.jpg")
                .unwrap_or_else(|error| panic!("URL: {error}")),
        ));
        let mut listing = ProductListing::rehydrate(RehydratedProductListingState {
            id: source.id,
            title_slug_id: source.title_slug_id,
            listing_source_id: source.listing_source_id,
            source_listing_id: source.source_listing_id,
            title: source.title,
            description: source.description,
            pricing: source.pricing,
            sale_observation: None,
            availability: source.availability,
            lifecycle: ListingLifecycle::Active,
            url: source.url,
            images: source.images,
            auction: source.auction,
        })
        .unwrap_or_else(|error| panic!("rehydrate: {error}"));
        let mut replacement = IndexSet::new();
        replacement.insert(ProductListingImage::new(
            Url::parse("https://shop.example/new.jpg")
                .unwrap_or_else(|error| panic!("URL: {error}")),
        ));

        listing
            .replace_images(replacement)
            .unwrap_or_else(|error| panic!("replace images: {error}"));

        let Some(ProductListingEventPayload::Changed(changed)) =
            listing.take_pending_event_payload()
        else {
            panic!("expected changed payload");
        };
        assert_eq!(None, changed.availability());
    }

    #[test]
    fn should_coalesce_lifecycle_changes_and_preserve_withdrawn_previous_availability() {
        let mut listing = rehydrated();
        listing
            .set_availability(ListingAvailability::SoldOut)
            .unwrap_or_else(|error| panic!("set availability: {error}"));
        assert_eq!(ChangeOutcome::Changed, listing.withdraw());

        let Some(ProductListingEventPayload::Changed(changed)) =
            listing.take_pending_event_payload()
        else {
            panic!("expected changed payload");
        };
        assert!(matches!(
            changed.lifecycle(),
            Some(ProductListingLifecycleChange::Withdrawn {
                previous_availability: Some(ListingAvailability::SoldOut)
            })
        ));
        assert_eq!(None, changed.availability());

        let mut listing = rehydrated();
        assert_eq!(ChangeOutcome::Changed, listing.withdraw());
        assert_eq!(ChangeOutcome::Changed, listing.restore());
        assert_eq!(None, listing.take_pending_event_payload());

        let withdrawn = input();
        let mut restored = ProductListing::rehydrate(RehydratedProductListingState {
            id: withdrawn.id,
            title_slug_id: withdrawn.title_slug_id,
            listing_source_id: withdrawn.listing_source_id,
            source_listing_id: withdrawn.source_listing_id,
            title: withdrawn.title,
            description: withdrawn.description,
            pricing: withdrawn.pricing,
            sale_observation: None,
            availability: None,
            lifecycle: ListingLifecycle::Withdrawn,
            url: withdrawn.url,
            images: withdrawn.images,
            auction: withdrawn.auction,
        })
        .unwrap_or_else(|error| panic!("rehydrate: {error}"));
        assert_eq!(ChangeOutcome::Changed, restored.restore());
        let Some(ProductListingEventPayload::Changed(changed)) =
            restored.take_pending_event_payload()
        else {
            panic!("expected restored payload");
        };
        assert!(matches!(
            changed.lifecycle(),
            Some(ProductListingLifecycleChange::Restored)
        ));
    }

    #[test]
    fn should_coalesce_sale_observations_and_retractions() {
        let mut listing = rehydrated();
        let observation = observation();
        listing
            .record_sale_observation(observation)
            .unwrap_or_else(|error| panic!("record sale: {error}"));
        let Some(ProductListingEventPayload::Changed(changed)) =
            listing.take_pending_event_payload()
        else {
            panic!("expected observed payload");
        };
        assert_eq!(
            Some(&ListingSaleObservationChange::Observed(observation)),
            changed.sale_observation()
        );

        assert_eq!(ChangeOutcome::Changed, listing.retract_sale_observation());
        let Some(ProductListingEventPayload::Changed(changed)) =
            listing.take_pending_event_payload()
        else {
            panic!("expected retracted payload");
        };
        assert_eq!(
            Some(&ListingSaleObservationChange::Retracted(observation)),
            changed.sale_observation()
        );

        let mut listing = rehydrated();
        listing
            .record_sale_observation(observation)
            .unwrap_or_else(|error| panic!("record sale: {error}"));
        assert_eq!(ChangeOutcome::Changed, listing.retract_sale_observation());
        assert_eq!(None, listing.take_pending_event_payload());
    }

    #[test]
    fn should_not_mutate_or_emit_for_invalid_auction() {
        let mut listing = rehydrated();
        let expected = listing.clone();
        let auction = ProductListingAuction {
            start: Some(OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(1)),
            end: Some(OffsetDateTime::UNIX_EPOCH),
        };

        assert_eq!(
            Err(ChangeProductListingError::AuctionStartAfterEnd),
            listing.replace_auction(auction)
        );
        assert_eq!(expected, listing);
        assert_eq!(None, listing.take_pending_event_payload());
    }

    #[test]
    fn should_reject_invalid_auction_during_creation_and_rehydration() {
        let auction = ProductListingAuction {
            start: Some(OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(1)),
            end: Some(OffsetDateTime::UNIX_EPOCH),
        };
        let mut creation_input = input();
        creation_input.auction = auction;
        assert_eq!(
            Err(RehydrateProductListingError::AuctionStartAfterEnd),
            ProductListing::create(creation_input)
        );

        let source = input();
        assert_eq!(
            Err(RehydrateProductListingError::AuctionStartAfterEnd),
            ProductListing::rehydrate(RehydratedProductListingState {
                id: source.id,
                title_slug_id: source.title_slug_id,
                listing_source_id: source.listing_source_id,
                source_listing_id: source.source_listing_id,
                title: source.title,
                description: source.description,
                pricing: source.pricing,
                sale_observation: None,
                availability: source.availability,
                lifecycle: ListingLifecycle::Active,
                url: source.url,
                images: source.images,
                auction,
            })
        );
    }

    #[test]
    fn should_reject_mutation_of_withdrawn_listing_without_pending_event() {
        let mut listing = rehydrated();
        listing.withdraw();
        listing.take_pending_event_payload();

        assert_eq!(
            Err(ChangeListingAvailabilityError::ListingWithdrawn),
            listing.set_availability(ListingAvailability::InStock)
        );
        assert_eq!(
            Err(ChangeProductListingError::ListingWithdrawn),
            listing.replace_pricing(ProductListingPricing::default())
        );
        assert_eq!(None, listing.take_pending_event_payload());
    }
}
