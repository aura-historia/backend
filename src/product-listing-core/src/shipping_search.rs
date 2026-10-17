//! Conservative advertised shipping predicates, independent of item location and browsing state.
use geo::{CountryCode, reference::ReferenceRelease};
use money::Price;
use shipping_core::terms::ShippingServiceCode;

#[derive(Clone, PartialEq, Eq)]
pub struct ShippingSearch {
    release: ReferenceRelease,
    country: Option<CountryCode>,
    service: Option<ShippingServiceCode>,
    maximum_per_item: Option<Price>,
    fx_converted: bool,
    collection_offered: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid whole-country shipping predicate")]
pub struct InvalidShippingSearch;

impl ShippingSearch {
    pub const INTERPRETATION: &str = "WHOLE_COUNTRY_PER_ITEM_V1";
    pub const FX_INTERPRETATION: &str = "WHOLE_COUNTRY_PER_ITEM_FX_V1";

    pub fn new(
        release: ReferenceRelease,
        country: Option<CountryCode>,
        service: Option<ShippingServiceCode>,
        maximum_per_item: Option<Price>,
        collection_offered: bool,
    ) -> Result<Self, InvalidShippingSearch> {
        if (country.is_none()
            && (!collection_offered || service.is_some() || maximum_per_item.is_some()))
            || country.is_some_and(|c| geo::country_from_code(c.alpha2(), release).is_err())
            || maximum_per_item.is_some_and(|p| u64::from(p.monetary_amount) > i64::MAX as u64)
        {
            return Err(InvalidShippingSearch);
        }
        Ok(Self {
            release,
            country,
            service,
            maximum_per_item,
            fx_converted: false,
            collection_offered,
        })
    }

    pub fn reference_release(&self) -> ReferenceRelease {
        self.release
    }
    /// Explicitly value the maximum in its declared target currency using the request's
    /// persisted FX snapshot. Destination and applicability semantics are unchanged.
    pub fn with_fx_conversion(mut self) -> Result<Self, InvalidShippingSearch> {
        if self.maximum_per_item.is_none() {
            return Err(InvalidShippingSearch);
        }
        self.fx_converted = true;
        Ok(self)
    }
    pub fn fx_converted(&self) -> bool {
        self.fx_converted
    }
    pub fn interpretation(&self) -> &'static str {
        if self.fx_converted {
            Self::FX_INTERPRETATION
        } else {
            Self::INTERPRETATION
        }
    }
    pub fn country(&self) -> Option<CountryCode> {
        self.country
    }
    pub fn service(&self) -> Option<&ShippingServiceCode> {
        self.service.as_ref()
    }
    pub fn maximum_per_item(&self) -> Option<Price> {
        self.maximum_per_item
    }
    pub fn collection_offered(&self) -> bool {
        self.collection_offered
    }
}

impl std::fmt::Debug for ShippingSearch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ShippingSearch(<private predicates>)")
    }
}
