use product_listing_core::listing_availability::ListingAvailability;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Distinguishes exact-value mappings from regex-pattern mappings.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StateMappingType {
    /// Exact string match (compared case-insensitively after trimming + lowercasing).
    Value,
    /// Regular-expression pattern (matched against the trimmed, lowercased input).
    Regex,
}

#[derive(Debug, Clone)]
pub struct ProductStateMappingRecord {
    /// Primary key — either a lowercased exact value or a regex pattern string.
    pub raw: String,
    pub normalized: Option<ListingAvailability>,
    pub mapping_type: StateMappingType,

    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}
