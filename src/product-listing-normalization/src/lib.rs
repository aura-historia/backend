//! Pure deterministic ProductListing normalization.
//!
//! Source integrations map provider fields into generic values before calling these APIs.
//! This crate performs no I/O, logging, configuration lookup, or provider interpretation.

pub mod availability;
pub mod date_time;
pub mod error;
pub mod image_url;
pub mod language;
pub mod price;
pub mod source_listing_id;
pub mod text;

pub use availability::{
    AvailabilityNormalizationError, ListingAvailabilityQuickCheck, quick_check_availability,
};
pub use date_time::{DateTimeNormalizationError, normalize_date_time};
pub use error::{DateTimeField, NormalizationError, PriceField};
pub use image_url::{ImageUrlNormalizationError, normalize_image_urls};
pub use language::detect_language;
pub use price::{PriceNormalizationError, normalize_price};
pub use source_listing_id::{
    SourceListingIdNormalizationError, normalize_source_listing_id_with_url_sha_fallback,
};
pub use text::{normalize_description, normalize_title};
