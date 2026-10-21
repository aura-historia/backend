use auction_core::AuctionSchedule;
use schemars::JsonSchema;
use serde::Serialize;
use time::OffsetDateTime;
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuctionScheduleResponseData {
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    bidding_opens: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    live_starts: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    lots_begin_closing: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    scheduled_end: Option<OffsetDateTime>,
}

impl From<AuctionSchedule> for AuctionScheduleResponseData {
    fn from(value: AuctionSchedule) -> Self {
        Self {
            bidding_opens: value.bidding_opens(),
            live_starts: value.live_starts(),
            lots_begin_closing: value.lots_begin_closing(),
            scheduled_end: value.scheduled_end(),
        }
    }
}
