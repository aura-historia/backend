use crate::description::Description;
use crate::listing_availability::ListingAvailability;
use crate::listing_lifecycle::ListingLifecycle;
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
    slug_id: ProductListingSlugId,
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
    pending_event_payloads: Vec<ProductListingEventPayload>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewProductListing {
    pub id: ProductListingId,
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
    pub slug_id: ProductListingSlugId,
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

#[derive(Debug, Clone, PartialEq)]
pub enum ProductListingEventPayload {
    Created(Box<ProductListingCreated>),
    AvailabilityChanged(ListingAvailabilityChanged),
    PriceChanged(ProductListingPriceChanged),
    UrlChanged(ProductListingUrlChanged),
    ImagesChanged(Box<ProductListingImagesChanged>),
    AuctionChanged(ProductListingAuctionChanged),
    Withdrawn(ProductListingWithdrawn),
    Restored(ProductListingRestored),
    SaleObserved(ListingSaleObserved),
    SaleObservationRetracted(ListingSaleObservationRetracted),
}

impl ProductListingEventPayload {
    pub const fn event_type(&self) -> &'static str {
        match self {
            Self::Created(_) => "PRODUCT_LISTING_CREATED",
            Self::AvailabilityChanged(_) => "PRODUCT_LISTING_AVAILABILITY_CHANGED",
            Self::PriceChanged(_) => "PRODUCT_LISTING_PRICE_CHANGED",
            Self::UrlChanged(_) => "PRODUCT_LISTING_URL_CHANGED",
            Self::ImagesChanged(_) => "PRODUCT_LISTING_IMAGES_CHANGED",
            Self::AuctionChanged(_) => "PRODUCT_LISTING_AUCTION_CHANGED",
            Self::Withdrawn(_) => "PRODUCT_LISTING_WITHDRAWN",
            Self::Restored(_) => "PRODUCT_LISTING_RESTORED",
            Self::SaleObserved(_) => "PRODUCT_LISTING_SALE_OBSERVED",
            Self::SaleObservationRetracted(_) => "PRODUCT_LISTING_SALE_OBSERVATION_RETRACTED",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingCreated {
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

#[derive(Debug, Clone, PartialEq)]
pub struct ListingAvailabilityChanged {
    pub previous: Option<ListingAvailability>,
    pub current: Option<ListingAvailability>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingPriceChanged {
    pub old_pricing: ProductListingPricing,
    pub new_pricing: ProductListingPricing,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingUrlChanged {
    pub old_url: Url,
    pub new_url: Url,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingImagesChanged {
    pub images: IndexSet<ProductListingImage>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingAuctionChanged {
    pub auction: ProductListingAuction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingWithdrawn {
    pub previous_availability: Option<ListingAvailability>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingRestored;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingSaleObserved {
    pub observation: ListingSaleObservation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingSaleObservationRetracted {
    pub observation: ListingSaleObservation,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RehydrateProductListingError {
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
    pub fn create(input: NewProductListing) -> Result<Self, RehydrateProductListingError> {
        let slug_id = product_listing_slug_id(input.id, input.title.as_ref());
        let mut listing = Self::rehydrate(RehydratedProductListingState {
            id: input.id,
            slug_id,
            listing_source_id: input.listing_source_id,
            source_listing_id: input.source_listing_id.clone(),
            title: input.title.clone(),
            description: input.description.clone(),
            pricing: input.pricing,
            sale_observation: None,
            availability: input.availability,
            lifecycle: ListingLifecycle::Active,
            url: input.url.clone(),
            images: input.images.clone(),
            auction: input.auction,
        })?;
        listing.push_event(ProductListingEventPayload::Created(Box::new(
            ProductListingCreated {
                listing_source_id: input.listing_source_id,
                source_listing_id: input.source_listing_id,
                title: input.title,
                description: input.description,
                pricing: input.pricing,
                availability: input.availability,
                url: input.url,
                images: input.images,
                auction: input.auction,
            },
        )));
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
            slug_id: state.slug_id,
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
            pending_event_payloads: Vec::new(),
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
        self.push_event(ProductListingEventPayload::Withdrawn(
            ProductListingWithdrawn {
                previous_availability,
            },
        ));
        ChangeOutcome::Changed
    }

    pub fn restore(&mut self) -> ChangeOutcome {
        if self.lifecycle == ListingLifecycle::Active {
            return ChangeOutcome::Unchanged;
        }
        self.lifecycle = ListingLifecycle::Active;
        self.availability = None;
        self.push_event(ProductListingEventPayload::Restored(ProductListingRestored));
        ChangeOutcome::Changed
    }

    pub fn record_sale_observation(
        &mut self,
        observation: ListingSaleObservation,
    ) -> Result<ChangeOutcome, RecordListingSaleObservationError> {
        match self.sale_observation {
            None => {
                self.sale_observation = Some(observation);
                self.push_event(ProductListingEventPayload::SaleObserved(
                    ListingSaleObserved { observation },
                ));
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
        self.push_event(ProductListingEventPayload::SaleObservationRetracted(
            ListingSaleObservationRetracted { observation },
        ));
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
        let old_pricing = self.pricing;
        self.pricing = pricing;
        self.push_event(ProductListingEventPayload::PriceChanged(
            ProductListingPriceChanged {
                old_pricing,
                new_pricing: pricing,
            },
        ));
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
        let old_url = self.url.clone();
        self.url = url.clone();
        self.push_event(ProductListingEventPayload::UrlChanged(
            ProductListingUrlChanged {
                old_url,
                new_url: url,
            },
        ));
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
        self.images = images.clone();
        self.push_event(ProductListingEventPayload::ImagesChanged(Box::new(
            ProductListingImagesChanged { images },
        )));
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
        self.auction = auction;
        self.push_event(ProductListingEventPayload::AuctionChanged(
            ProductListingAuctionChanged { auction },
        ));
        Ok(ChangeOutcome::Changed)
    }

    pub fn pending_event_payloads(&self) -> &[ProductListingEventPayload] {
        &self.pending_event_payloads
    }

    pub fn take_pending_event_payloads(&mut self) -> Vec<ProductListingEventPayload> {
        std::mem::take(&mut self.pending_event_payloads)
    }

    pub fn id(&self) -> ProductListingId {
        self.id
    }
    pub fn slug_id(&self) -> &ProductListingSlugId {
        &self.slug_id
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
        self.push_event(ProductListingEventPayload::AvailabilityChanged(
            ListingAvailabilityChanged { previous, current },
        ));
    }

    fn push_event(&mut self, payload: ProductListingEventPayload) {
        self.pending_event_payloads.push(payload);
    }
}

fn product_listing_slug_id(
    product_listing_id: ProductListingId,
    title: Option<&Localized<Language, Title>>,
) -> ProductListingSlugId {
    match title {
        Some(title) => ProductListingSlugId::from(title.payload.as_ref()),
        None => ProductListingSlugId::from(product_listing_id.to_string()),
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

    fn input() -> NewProductListing {
        NewProductListing {
            id: ProductListingId::new(),
            listing_source_id: ListingSourceId::new(),
            source_listing_id: SourceListingId::new(),
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

    #[test]
    fn should_create_active_listing_without_availability_assertion() {
        let input = input();
        let listing_source_id = input.listing_source_id;
        let source_listing_id = input.source_listing_id.clone();
        let listing =
            ProductListing::create(input).unwrap_or_else(|error| panic!("create: {error}"));
        assert_eq!(ListingLifecycle::Active, listing.lifecycle());
        assert_eq!(None, listing.availability());
        let [ProductListingEventPayload::Created(created)] = listing.pending_event_payloads()
        else {
            panic!("expected product listing created event");
        };
        assert_eq!(listing_source_id, created.listing_source_id);
        assert_eq!(&source_listing_id, &created.source_listing_id);
    }

    #[test]
    fn should_set_and_clear_availability_with_events() {
        let mut listing =
            ProductListing::create(input()).unwrap_or_else(|error| panic!("create: {error}"));
        listing.take_pending_event_payloads();
        assert_eq!(
            Ok(ChangeOutcome::Changed),
            listing.set_availability(ListingAvailability::InStock)
        );
        assert_eq!(Some(ListingAvailability::InStock), listing.availability());
        assert_eq!(Ok(ChangeOutcome::Changed), listing.clear_availability());
        assert_eq!(None, listing.availability());
    }

    #[test]
    fn should_withdraw_clear_availability_and_restore_without_assertion() {
        let mut listing =
            ProductListing::create(input()).unwrap_or_else(|error| panic!("create: {error}"));
        listing.take_pending_event_payloads();
        listing
            .set_availability(ListingAvailability::SoldOut)
            .unwrap_or_else(|error| panic!("set: {error}"));
        assert_eq!(ChangeOutcome::Changed, listing.withdraw());
        assert_eq!(ListingLifecycle::Withdrawn, listing.lifecycle());
        assert_eq!(None, listing.availability());
        assert_eq!(ChangeOutcome::Changed, listing.restore());
        assert_eq!(ListingLifecycle::Active, listing.lifecycle());
        assert_eq!(None, listing.availability());
    }

    #[rstest::rstest]
    #[case(None, Some(100), ChangeOutcome::Changed)]
    #[case(Some(100), Some(120), ChangeOutcome::Changed)]
    #[case(Some(100), Some(100), ChangeOutcome::Unchanged)]
    #[case(Some(100), None, ChangeOutcome::Changed)]
    #[case(None, None, ChangeOutcome::Unchanged)]
    fn should_set_and_clear_current_price_with_expected_events(
        #[case] current_amount: Option<u64>,
        #[case] requested_amount: Option<u64>,
        #[case] expected_outcome: ChangeOutcome,
    ) {
        let mut source = input();
        source.pricing.price = current_amount.map(eur_price);
        let mut listing =
            ProductListing::create(source).unwrap_or_else(|error| panic!("create: {error}"));
        listing.take_pending_event_payloads();

        let outcome = match requested_amount {
            Some(amount) => listing.set_price(eur_price(amount)),
            None => listing.clear_price(),
        };

        assert_eq!(Ok(expected_outcome), outcome);
        assert_eq!(requested_amount.map(eur_price), listing.pricing().price);
        match expected_outcome {
            ChangeOutcome::Changed => assert!(matches!(
                listing.pending_event_payloads(),
                [ProductListingEventPayload::PriceChanged(ProductListingPriceChanged {
                    old_pricing,
                    new_pricing,
                })]
                    if old_pricing.price == current_amount.map(eur_price)
                        && new_pricing.price == requested_amount.map(eur_price)
            )),
            ChangeOutcome::Unchanged => assert!(listing.pending_event_payloads().is_empty()),
        }
    }

    fn eur_price(amount: u64) -> Price {
        Price::new(money::MonetaryAmount::from(amount), money::Currency::Eur)
    }

    #[test]
    fn should_reject_mutation_of_withdrawn_listing() {
        let mut listing =
            ProductListing::create(input()).unwrap_or_else(|error| panic!("create: {error}"));
        listing.withdraw();
        assert_eq!(
            Err(ChangeListingAvailabilityError::ListingWithdrawn),
            listing.set_availability(ListingAvailability::InStock)
        );
        assert_eq!(
            Err(ChangeProductListingError::ListingWithdrawn),
            listing.replace_pricing(ProductListingPricing::default())
        );
    }

    #[test]
    fn should_reject_rehydrated_withdrawn_listing_with_availability() {
        let source = input();
        let result = ProductListing::rehydrate(RehydratedProductListingState {
            id: source.id,
            slug_id: product_listing_slug_id(source.id, source.title.as_ref()),
            listing_source_id: source.listing_source_id,
            source_listing_id: source.source_listing_id,
            title: source.title,
            description: source.description,
            pricing: source.pricing,
            sale_observation: None,
            availability: Some(ListingAvailability::Available),
            lifecycle: ListingLifecycle::Withdrawn,
            url: source.url,
            images: source.images,
            auction: source.auction,
        });
        assert_eq!(
            Err(RehydrateProductListingError::WithdrawnListingHasAvailability),
            result
        );
    }

    #[test]
    fn should_not_emit_event_for_idempotent_lifecycle_or_availability_operations() {
        let mut listing =
            ProductListing::create(input()).unwrap_or_else(|error| panic!("create: {error}"));
        listing.take_pending_event_payloads();
        assert_eq!(Ok(ChangeOutcome::Unchanged), listing.clear_availability());
        assert_eq!(ChangeOutcome::Changed, listing.withdraw());
        listing.take_pending_event_payloads();
        assert_eq!(ChangeOutcome::Unchanged, listing.withdraw());
        assert!(listing.pending_event_payloads().is_empty());
    }

    #[test]
    fn should_reject_invalid_auction_without_mutating_listing_or_events() {
        let mut listing =
            ProductListing::create(input()).unwrap_or_else(|error| panic!("create: {error}"));
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
                slug_id: product_listing_slug_id(source.id, source.title.as_ref()),
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

    fn observation() -> ListingSaleObservation {
        ListingSaleObservation::new(OffsetDateTime::UNIX_EPOCH, FxRateId::new())
    }

    #[test]
    fn should_record_sale_observation_without_changing_listing_facts() {
        let mut listing =
            ProductListing::create(input()).unwrap_or_else(|error| panic!("create: {error}"));
        listing.take_pending_event_payloads();
        let observation = observation();

        assert_eq!(
            Ok(ChangeOutcome::Changed),
            listing.record_sale_observation(observation)
        );
        assert_eq!(Some(observation), listing.sale_observation());
        assert_eq!(ListingLifecycle::Active, listing.lifecycle());
        assert_eq!(None, listing.availability());
        assert!(matches!(
            listing.pending_event_payloads(),
            [ProductListingEventPayload::SaleObserved(ListingSaleObserved {
                observation: observed,
            })] if *observed == observation
        ));
    }

    #[test]
    fn should_make_same_sale_observation_idempotent_and_reject_conflict() {
        let mut listing =
            ProductListing::create(input()).unwrap_or_else(|error| panic!("create: {error}"));
        listing.take_pending_event_payloads();
        let observation = observation();
        listing
            .record_sale_observation(observation)
            .unwrap_or_else(|error| panic!("record: {error}"));
        listing.take_pending_event_payloads();

        assert_eq!(
            Ok(ChangeOutcome::Unchanged),
            listing.record_sale_observation(observation)
        );
        assert_eq!(
            Err(RecordListingSaleObservationError::ConflictingExistingObservation),
            listing.record_sale_observation(ListingSaleObservation::new(
                OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(1),
                FxRateId::new(),
            ))
        );
        assert_eq!(Some(observation), listing.sale_observation());
        assert!(listing.pending_event_payloads().is_empty());
    }

    #[test]
    fn should_retract_sale_observation_idempotently_and_preserve_it_across_lifecycle_changes() {
        let mut listing =
            ProductListing::create(input()).unwrap_or_else(|error| panic!("create: {error}"));
        listing.take_pending_event_payloads();
        let observation = observation();
        listing
            .record_sale_observation(observation)
            .unwrap_or_else(|error| panic!("record: {error}"));
        listing.withdraw();
        listing.restore();
        assert_eq!(Some(observation), listing.sale_observation());
        listing.take_pending_event_payloads();

        assert_eq!(ChangeOutcome::Changed, listing.retract_sale_observation());
        assert_eq!(None, listing.sale_observation());
        assert!(matches!(
            listing.pending_event_payloads(),
            [ProductListingEventPayload::SaleObservationRetracted(
                ListingSaleObservationRetracted { observation: retracted }
            )] if *retracted == observation
        ));
        listing.take_pending_event_payloads();
        assert_eq!(ChangeOutcome::Unchanged, listing.retract_sale_observation());
        assert!(listing.pending_event_payloads().is_empty());
    }
}
