//! Reviewed per-source description boilerplate (`docs/product-listing.md#description-boilerplate`).
//!
//! Operators review candidate sentences proposed by `normalization-replay boilerplate
//! candidates`. `STRIP` sentences drive stripping during raw normalization; `KEEP` sentences were
//! reviewed as item facts and are never proposed again.

use application::error::BoxError;
use async_trait::async_trait;
use listing_source_core::ListingSourceId;
use product_listing_normalization::{BoilerplateSentence, DescriptionBoilerplate};
use product_listing_service::ports::ProductListingRawStreamId;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter)]
pub enum DescriptionBoilerplateDecision {
    /// Shop boilerplate: stripped from the source's descriptions.
    Strip,
    /// Reviewed as an item fact, such as whether a postcard was posted: never stripped.
    Keep,
}

impl DescriptionBoilerplateDecision {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Strip => "STRIP",
            Self::Keep => "KEEP",
        }
    }

    pub fn from_code(value: &str) -> Option<Self> {
        Self::iter().find(|decision| decision.as_str() == value)
    }
}

/// One reviewer decision about one sentence of one listing source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescriptionBoilerplateReview {
    pub listing_source_id: ListingSourceId,
    pub sentence: BoilerplateSentence,
    pub decision: DescriptionBoilerplateDecision,
}

#[derive(Debug, thiserror::Error)]
pub enum DescriptionBoilerplatePortError {
    #[error("description boilerplate persistence failed")]
    Persistence {
        #[source]
        source: BoxError,
    },
    #[error("description boilerplate persisted state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait]
pub trait DescriptionBoilerplateReader: Send + Sync {
    /// The source's `STRIP` sentences; empty when none were reviewed.
    async fn strip_set(
        &self,
        listing_source_id: ListingSourceId,
    ) -> Result<DescriptionBoilerplate, DescriptionBoilerplatePortError>;

    /// Every review of the selected source, or of all sources.
    async fn reviews(
        &self,
        listing_source_id: Option<ListingSourceId>,
    ) -> Result<Vec<DescriptionBoilerplateReview>, DescriptionBoilerplatePortError>;
}

/// Records reviews atomically: a later decision for the same source and sentence replaces the
/// earlier one.
#[async_trait]
pub trait DescriptionBoilerplateReviewWriter: Send + Sync {
    async fn record(
        &self,
        reviews: Vec<DescriptionBoilerplateReview>,
    ) -> Result<(), DescriptionBoilerplatePortError>;
}

/// The description of a listing's latest raw revision, the unit candidate analysis counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentRawDescription {
    pub listing_source_id: ListingSourceId,
    pub product_listing_raw_stream_id: ProductListingRawStreamId,
    /// `None` when the retained value is not a list of strings, which normalization rejects.
    pub fragments: Option<Vec<String>>,
}

/// Keyset position in the `(listing source, raw stream)` order of current raw descriptions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentRawDescriptionCursor {
    pub listing_source_id: ListingSourceId,
    pub product_listing_raw_stream_id: ProductListingRawStreamId,
}

/// Pages the set descriptions of every stream's latest UPSERT revision, ordered by listing source
/// so analysis can finish one source before reading the next.
#[async_trait]
pub trait CurrentRawDescriptionReader: Send + Sync {
    async fn page(
        &self,
        listing_source_id: Option<ListingSourceId>,
        after: Option<CurrentRawDescriptionCursor>,
        limit: u32,
    ) -> Result<Vec<CurrentRawDescription>, DescriptionBoilerplatePortError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_round_trip_persisted_decision_codes() {
        for decision in DescriptionBoilerplateDecision::iter() {
            assert_eq!(
                Some(decision),
                DescriptionBoilerplateDecision::from_code(decision.as_str())
            );
        }
        assert_eq!(None, DescriptionBoilerplateDecision::from_code("strip"));
    }
}
