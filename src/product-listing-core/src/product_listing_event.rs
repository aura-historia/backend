use crate::description::Description;
use crate::listing_availability::ListingAvailability;

use crate::product_listing::{
    ListingSaleObservation, ProductListingAuction, ProductListingPricing,
};
use crate::source_listing_id::SourceListingId;
use crate::title::Title;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use money::Price;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::EnumIter)]
pub enum ProductListingEventType {
    Discovered,
    Changed,
}

impl ProductListingEventType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "PRODUCT_LISTING_DISCOVERED",
            Self::Changed => "PRODUCT_LISTING_CHANGED",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProductListingEventPayload {
    Discovered(ProductListingDiscovered),
    Changed(ProductListingChanged),
}

impl ProductListingEventPayload {
    pub const fn event_type(&self) -> ProductListingEventType {
        match self {
            Self::Discovered(_) => ProductListingEventType::Discovered,
            Self::Changed(_) => ProductListingEventType::Changed,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingDiscovered {
    listing_source_id: ListingSourceId,
    source_listing_id: SourceListingId,
    title: Option<Localized<Language, Title>>,
    description: Option<Localized<Language, Description>>,
    pricing: ProductListingPricing,
    availability: Option<ListingAvailability>,
    url: Url,
    image_count: usize,
    auction: ProductListingAuction,
}

impl ProductListingDiscovered {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        listing_source_id: ListingSourceId,
        source_listing_id: SourceListingId,
        title: Option<Localized<Language, Title>>,
        description: Option<Localized<Language, Description>>,
        pricing: ProductListingPricing,
        availability: Option<ListingAvailability>,
        url: Url,
        image_count: usize,
        auction: ProductListingAuction,
    ) -> Self {
        Self {
            listing_source_id,
            source_listing_id,
            title,
            description,
            pricing,
            availability,
            url,
            image_count,
            auction,
        }
    }

    pub const fn listing_source_id(&self) -> ListingSourceId {
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

    pub const fn pricing(&self) -> ProductListingPricing {
        self.pricing
    }

    pub const fn availability(&self) -> Option<ListingAvailability> {
        self.availability
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub const fn image_count(&self) -> usize {
        self.image_count
    }

    pub const fn auction(&self) -> ProductListingAuction {
        self.auction
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ValueChange<T> {
    previous: T,
    current: T,
}

impl<T> ValueChange<T> {
    pub(crate) const fn new(previous: T, current: T) -> Self {
        Self { previous, current }
    }

    pub const fn previous(&self) -> &T {
        &self.previous
    }

    pub const fn current(&self) -> &T {
        &self.current
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProductListingLifecycleChange {
    Withdrawn {
        previous_availability: Option<ListingAvailability>,
    },
    Restored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingSaleObservationChange {
    Observed(ListingSaleObservation),
    Retracted(ListingSaleObservation),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingChanged {
    price: Option<ValueChange<Option<Price>>>,
    price_estimate_min: Option<ValueChange<Option<Price>>>,
    price_estimate_max: Option<ValueChange<Option<Price>>>,
    availability: Option<ValueChange<Option<ListingAvailability>>>,
    url: Option<ValueChange<Url>>,
    image_count: Option<ValueChange<usize>>,
    auction: Option<ValueChange<ProductListingAuction>>,
    lifecycle: Option<ProductListingLifecycleChange>,
    sale_observation: Option<ListingSaleObservationChange>,
}

impl ProductListingChanged {
    pub(crate) const fn empty() -> Self {
        Self {
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            availability: None,
            url: None,
            image_count: None,
            auction: None,
            lifecycle: None,
            sale_observation: None,
        }
    }

    pub(crate) fn change_price(&mut self, previous: Option<Price>, current: Option<Price>) {
        coalesce_value_change(&mut self.price, previous, current, false);
    }

    pub(crate) fn change_price_estimate_min(
        &mut self,
        previous: Option<Price>,
        current: Option<Price>,
    ) {
        coalesce_value_change(&mut self.price_estimate_min, previous, current, false);
    }

    pub(crate) fn change_price_estimate_max(
        &mut self,
        previous: Option<Price>,
        current: Option<Price>,
    ) {
        coalesce_value_change(&mut self.price_estimate_max, previous, current, false);
    }

    pub(crate) fn change_availability(
        &mut self,
        previous: Option<ListingAvailability>,
        current: Option<ListingAvailability>,
    ) {
        coalesce_value_change(&mut self.availability, previous, current, false);
    }

    pub(crate) fn change_url(&mut self, previous: Url, current: Url) {
        coalesce_value_change(&mut self.url, previous, current, false);
    }

    pub(crate) fn change_image_count(&mut self, previous: usize, current: usize) {
        coalesce_value_change(&mut self.image_count, previous, current, true);
    }

    pub(crate) fn change_auction(
        &mut self,
        previous: ProductListingAuction,
        current: ProductListingAuction,
    ) {
        coalesce_value_change(&mut self.auction, previous, current, false);
    }

    pub(crate) fn change_lifecycle(&mut self, change: ProductListingLifecycleChange) {
        self.lifecycle = match (&self.lifecycle, change) {
            (
                Some(ProductListingLifecycleChange::Withdrawn { .. }),
                ProductListingLifecycleChange::Restored,
            )
            | (
                Some(ProductListingLifecycleChange::Restored),
                ProductListingLifecycleChange::Withdrawn { .. },
            ) => None,
            (_, change) => Some(change),
        };
    }

    pub(crate) fn change_sale_observation(&mut self, change: ListingSaleObservationChange) {
        self.sale_observation = match (&self.sale_observation, change) {
            (
                Some(ListingSaleObservationChange::Observed(previous)),
                ListingSaleObservationChange::Retracted(current),
            ) if previous == &current => None,
            (_, change) => Some(change),
        };
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.price.is_none()
            && self.price_estimate_min.is_none()
            && self.price_estimate_max.is_none()
            && self.availability.is_none()
            && self.url.is_none()
            && self.image_count.is_none()
            && self.auction.is_none()
            && self.lifecycle.is_none()
            && self.sale_observation.is_none()
    }

    pub const fn price(&self) -> Option<&ValueChange<Option<Price>>> {
        self.price.as_ref()
    }

    pub const fn price_estimate_min(&self) -> Option<&ValueChange<Option<Price>>> {
        self.price_estimate_min.as_ref()
    }

    pub const fn price_estimate_max(&self) -> Option<&ValueChange<Option<Price>>> {
        self.price_estimate_max.as_ref()
    }

    pub const fn availability(&self) -> Option<&ValueChange<Option<ListingAvailability>>> {
        self.availability.as_ref()
    }

    pub const fn url(&self) -> Option<&ValueChange<Url>> {
        self.url.as_ref()
    }

    pub const fn image_count(&self) -> Option<&ValueChange<usize>> {
        self.image_count.as_ref()
    }

    pub const fn auction(&self) -> Option<&ValueChange<ProductListingAuction>> {
        self.auction.as_ref()
    }

    pub const fn lifecycle(&self) -> Option<&ProductListingLifecycleChange> {
        self.lifecycle.as_ref()
    }

    pub const fn sale_observation(&self) -> Option<&ListingSaleObservationChange> {
        self.sale_observation.as_ref()
    }
}

fn coalesce_value_change<T: Clone + PartialEq>(
    change: &mut Option<ValueChange<T>>,
    previous: T,
    current: T,
    preserve_equal: bool,
) {
    let first_previous = change
        .as_ref()
        .map_or(previous, |existing| existing.previous.clone());

    if !preserve_equal && first_previous == current {
        *change = None;
        return;
    }

    *change = Some(ValueChange::new(first_previous, current));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use strum::IntoEnumIterator;

    #[test]
    fn should_use_only_unique_canonical_product_listing_event_types() {
        let event_types = ProductListingEventType::iter()
            .map(ProductListingEventType::as_str)
            .collect::<HashSet<_>>();

        assert_eq!(2, event_types.len());
        assert!(event_types.contains("PRODUCT_LISTING_DISCOVERED"));
        assert!(event_types.contains("PRODUCT_LISTING_CHANGED"));
    }

    #[test]
    fn should_keep_image_count_change_when_counts_are_equal() {
        let mut changed = ProductListingChanged::empty();

        changed.change_image_count(2, 2);

        assert_eq!(Some(&2), changed.image_count().map(ValueChange::previous));
        assert_eq!(Some(&2), changed.image_count().map(ValueChange::current));
        assert!(!changed.is_empty());
    }
}
