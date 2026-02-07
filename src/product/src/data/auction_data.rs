use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Default, Serialize, Deserialize)]
pub struct AuctionData {
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub start: Option<OffsetDateTime>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub end: Option<OffsetDateTime>,
}
