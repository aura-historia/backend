use product_core::product_state::ProductState;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductStateMappingRecord {
    /// Primary key — either a lowercased exact value or a regex pattern string.
    pub raw: String,
    pub normalized: ProductState,
    pub mapping_type: StateMappingType,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}
