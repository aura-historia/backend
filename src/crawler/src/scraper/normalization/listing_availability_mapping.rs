use product_listing_core::listing_availability::ListingAvailability;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Crawler-local availability decision. It never becomes a core enum value.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ListingAvailabilityMapping {
    Availability(ListingAvailability),
    NoAssertion,
    Ignore,
}

impl ListingAvailabilityMapping {
    pub const fn availability(self) -> Option<ListingAvailability> {
        match self {
            Self::Availability(availability) => Some(availability),
            Self::NoAssertion | Self::Ignore => None,
        }
    }

    pub const fn is_persistable(self) -> bool {
        !matches!(self, Self::Ignore)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ListingAvailabilityMappingType {
    Value,
    Regex,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ListingAvailabilityDecisionKind {
    Availability,
    NoAssertion,
}

impl ListingAvailabilityDecisionKind {
    pub const fn from_mapping(mapping: ListingAvailabilityMapping) -> Option<Self> {
        match mapping {
            ListingAvailabilityMapping::Availability(_) => Some(Self::Availability),
            ListingAvailabilityMapping::NoAssertion => Some(Self::NoAssertion),
            ListingAvailabilityMapping::Ignore => None,
        }
    }
}

/// Durable mapping record. `Ignore` is intentionally not representable here.
#[derive(Debug, Clone)]
pub struct ListingAvailabilityMappingRecord {
    pub raw: String,
    pub availability: Option<ListingAvailability>,
    pub mapping_type: ListingAvailabilityMappingType,
    pub decision_kind: ListingAvailabilityDecisionKind,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl ListingAvailabilityMappingRecord {
    pub const fn mapping(&self) -> ListingAvailabilityMapping {
        match self.decision_kind {
            ListingAvailabilityDecisionKind::Availability => match self.availability {
                Some(availability) => ListingAvailabilityMapping::Availability(availability),
                None => ListingAvailabilityMapping::NoAssertion,
            },
            ListingAvailabilityDecisionKind::NoAssertion => ListingAvailabilityMapping::NoAssertion,
        }
    }
}
