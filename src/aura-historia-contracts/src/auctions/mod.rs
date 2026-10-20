//! Shared public auction wire shapes and pure cursor/request mappings.
mod catalogue;
mod detail;
mod directory;
mod schedule;
use application::error::{BoxError, box_error};
use auction_core::{AuctionFormat, AuctionReportedStatus};
pub use catalogue::*;
pub use detail::*;
pub use directory::*;
pub use schedule::AuctionScheduleResponseData;
use schemars::{Schema, SchemaGenerator, json_schema};
use strum::IntoEnumIterator;

#[derive(Debug, thiserror::Error)]
pub enum AuctionQueryError {
    #[error("{0}")]
    InvalidQuery(&'static str),
    #[error("invalid auction continuation cursor")]
    InvalidCursor(#[source] serde_json::Error),
    #[error("must be a valid {semantic_type} ID")]
    InvalidObjectId {
        field: &'static str,
        semantic_type: &'static str,
        #[source]
        source: BoxError,
    },
}
fn parse_query_object_id<T: std::str::FromStr>(
    value: &str,
    field: &'static str,
    semantic_type: &'static str,
) -> Result<T, AuctionQueryError>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    value
        .parse()
        .map_err(|source| AuctionQueryError::InvalidObjectId {
            field,
            semantic_type,
            source: box_error(source),
        })
}
fn optional_order_schema(_: &mut SchemaGenerator) -> Schema {
    optional_codes(["asc", "desc"])
}

fn optional_codes(codes: impl IntoIterator<Item = &'static str>) -> Schema {
    let codes = codes.into_iter().collect::<Vec<_>>();
    json_schema!({"anyOf":[{"type":"string","enum":codes},{"type":"null"}]})
}
fn format_schema(_: &mut SchemaGenerator) -> Schema {
    optional_codes(AuctionFormat::iter().map(AuctionFormat::as_str))
}
fn status_schema(_: &mut SchemaGenerator) -> Schema {
    optional_codes(AuctionReportedStatus::iter().map(AuctionReportedStatus::as_str))
}
