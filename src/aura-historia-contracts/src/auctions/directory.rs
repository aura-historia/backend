use super::PublicSourceData;
use super::{AuctionQueryError, parse_query_object_id};
use application::pagination::Cursor;
use auction_core::{AuctionFormat, AuctionId, AuctionReportedStatus, AuctionSchedulePoint};
use auction_service::{
    ports::{
        AuctionDirectoryCursor, AuctionDirectoryScope, AuctionInstantScheduleFilter,
        ListAuctionsDirectoryRequest,
    },
    use_cases::ListAuctionsRequest,
};
use domain_primitives::sort::SortOrder;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectoryQuery {
    #[schemars(schema_with = "crate::wire::schema::optional_listing_source_id")]
    pub listing_source_id: Option<String>,
    #[schemars(schema_with = "super::format_schema")]
    pub format: Option<String>,
    #[schemars(schema_with = "super::status_schema")]
    pub reported_status: Option<String>,
    #[schemars(schema_with = "schedule_role_schema")]
    pub time_role: Option<String>,
    #[schemars(schema_with = "sort_schema")]
    pub sort: Option<String>,
    #[schemars(schema_with = "super::optional_order_schema")]
    pub order: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    pub from: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    pub to: Option<OffsetDateTime>,
    pub page_size: Option<u64>,
    pub search_after: Option<String>,
}

impl DirectoryQuery {
    pub fn try_into_request(self) -> Result<ListAuctionsRequest, AuctionQueryError> {
        let listing_source_id = self
            .listing_source_id
            .map(|value| parse_query_object_id(&value, "listingSourceId", "ListingSource"))
            .transpose()?;
        let format = parse_auction_format(self.format)?;
        let reported_status = parse_auction_status(self.reported_status)?;
        let role = self.time_role.as_deref().map(schedule_point).transpose()?;
        let sort = parse_directory_sort(self.sort.as_deref(), role)?;
        let order = self
            .order
            .map(|value| parse_directory_order(&value))
            .transpose()?;
        let schedule = parse_schedule_filter(role, self.from, self.to, sort.is_some())?;
        let cursor = self.search_after.map(parse_directory_cursor).transpose()?;
        Ok(ListAuctionsDirectoryRequest {
            listing_source_id,
            format,
            reported_status,
            schedule,
            sort,
            order,
            cursor: Some(Cursor {
                size: self.page_size.unwrap_or(21),
                search_after: cursor,
            }),
        })
    }
}

fn schedule_point(value: &str) -> Result<AuctionSchedulePoint, AuctionQueryError> {
    match value {
        "BIDDING_OPENS" => Ok(AuctionSchedulePoint::BiddingOpens),
        "LIVE_STARTS" => Ok(AuctionSchedulePoint::LiveStarts),
        "LOTS_BEGIN_CLOSING" => Ok(AuctionSchedulePoint::LotsBeginClosing),
        "SCHEDULED_END" => Ok(AuctionSchedulePoint::ScheduledEnd),
        _ => Err(query_error("timeRole is invalid.")),
    }
}

fn parse_directory_sort(
    value: Option<&str>,
    role: Option<AuctionSchedulePoint>,
) -> Result<Option<AuctionSchedulePoint>, AuctionQueryError> {
    match value.unwrap_or("created") {
        "created" => Ok(None),
        "scheduled" => role
            .map(Some)
            .ok_or_else(|| query_error("sort=scheduled requires timeRole.")),
        _ => Err(query_error("sort must be created or scheduled.")),
    }
}

fn parse_directory_order(value: &str) -> Result<SortOrder, AuctionQueryError> {
    SortOrder::try_from(value).map_err(|_| query_error("order must be asc or desc."))
}

fn query_error(detail: &'static str) -> AuctionQueryError {
    AuctionQueryError::InvalidQuery(detail)
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectoryCursorData {
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(schema_with = "crate::wire::schema::datetime")]
    pub created: OffsetDateTime,
    #[schemars(schema_with = "crate::wire::schema::auction_id")]
    pub auction_id: String,
    // The outer Option distinguishes a missing field from a null schedule instant.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_nullable_scheduled",
        deserialize_with = "deserialize_nullable_scheduled"
    )]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    pub scheduled: Option<Option<OffsetDateTime>>,
    pub scope: DirectoryCursorScopeData,
}

fn serialize_nullable_scheduled<S: serde::Serializer>(
    value: &Option<Option<OffsetDateTime>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let scheduled = value.as_ref().copied().flatten();
    time::serde::rfc3339::option::serialize(&scheduled, serializer)
}

fn deserialize_nullable_scheduled<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<OffsetDateTime>>, D::Error> {
    time::serde::rfc3339::option::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectoryCursorScopeData {
    #[schemars(schema_with = "crate::wire::schema::optional_listing_source_id")]
    pub listing_source_id: Option<String>,
    #[schemars(schema_with = "super::format_schema")]
    pub format: Option<String>,
    #[schemars(schema_with = "super::status_schema")]
    pub reported_status: Option<String>,
    #[schemars(schema_with = "schedule_role_schema")]
    pub time_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "sort_schema")]
    pub sort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "super::optional_order_schema")]
    pub order: Option<SortOrder>,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    pub from: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    pub to: Option<OffsetDateTime>,
}

impl DirectoryCursorData {
    fn try_into_cursor(self) -> Result<AuctionDirectoryCursor, AuctionQueryError> {
        let scope = self.scope.try_into_scope()?;
        if scope.sort.is_some() != self.scheduled.is_some() {
            return Err(query_error(
                "searchAfter scheduled instant does not match its sort scope.",
            ));
        }
        Ok(AuctionDirectoryCursor {
            created: self.created,
            auction_id: parse_query_object_id(
                &self.auction_id,
                "searchAfter.auctionId",
                "Auction",
            )?,
            scheduled: self.scheduled.flatten(),
            scope,
        })
    }
}

pub fn parse_directory_cursor(value: String) -> Result<AuctionDirectoryCursor, AuctionQueryError> {
    serde_json::from_str::<DirectoryCursorData>(&value)
        .map_err(AuctionQueryError::InvalidCursor)?
        .try_into_cursor()
}

impl DirectoryCursorScopeData {
    fn try_into_scope(self) -> Result<AuctionDirectoryScope, AuctionQueryError> {
        let role = self.time_role.as_deref().map(schedule_point).transpose()?;
        let sort = parse_directory_sort(self.sort.as_deref(), role)?;
        let order = match (sort, self.order) {
            (Some(_), None) => {
                return Err(query_error("scheduled searchAfter scope requires order."));
            }
            (None, Some(SortOrder::Desc)) => None,
            (_, order) => order,
        };
        Ok(AuctionDirectoryScope {
            listing_source_id: self
                .listing_source_id
                .map(|value| {
                    parse_query_object_id(
                        &value,
                        "searchAfter.scope.listingSourceId",
                        "ListingSource",
                    )
                })
                .transpose()?,
            format: parse_auction_format(self.format)?,
            reported_status: parse_auction_status(self.reported_status)?,
            schedule: parse_schedule_filter(role, self.from, self.to, sort.is_some())?,
            sort,
            order,
        })
    }
}

impl From<AuctionDirectoryScope> for DirectoryCursorScopeData {
    fn from(value: AuctionDirectoryScope) -> Self {
        let (time_role, from, to) = match value.schedule {
            Some(schedule) => (
                Some(schedule_role_code(schedule.role).to_owned()),
                schedule.range.min,
                schedule.range.max,
            ),
            None => (
                value.sort.map(|role| schedule_role_code(role).to_owned()),
                None,
                None,
            ),
        };
        Self {
            listing_source_id: value.listing_source_id.map(|value| value.to_string()),
            format: value.format.map(|value| value.as_str().to_owned()),
            reported_status: value.reported_status.map(|value| value.as_str().to_owned()),
            time_role,
            sort: value.sort.map(|_| "scheduled".to_owned()),
            order: value.order,
            from,
            to,
        }
    }
}

fn parse_auction_format(value: Option<String>) -> Result<Option<AuctionFormat>, AuctionQueryError> {
    value
        .map(|value| {
            value
                .parse()
                .map_err(|_| query_error("format must be LIVE or TIMED."))
        })
        .transpose()
}

fn parse_auction_status(
    value: Option<String>,
) -> Result<Option<AuctionReportedStatus>, AuctionQueryError> {
    value
        .map(|value| {
            value
                .parse()
                .map_err(|_| query_error("reportedStatus is invalid."))
        })
        .transpose()
}

fn parse_schedule_filter(
    time_role: Option<AuctionSchedulePoint>,
    from: Option<OffsetDateTime>,
    to: Option<OffsetDateTime>,
    allow_unbounded: bool,
) -> Result<Option<AuctionInstantScheduleFilter>, AuctionQueryError> {
    match (time_role, from, to) {
        (None, None, None) => Ok(None),
        (Some(_), None, None) if allow_unbounded => Ok(None),
        (Some(role), Some(from), Some(to)) => Ok(Some(AuctionInstantScheduleFilter {
            role,
            range: domain_primitives::query::range_query::RangeQuery {
                min: Some(from),
                max: Some(to),
            },
        })),
        _ => Err(query_error(
            "timeRole, from, and to must be supplied together unless sorting by scheduled.",
        )),
    }
}

fn schedule_role_code(value: AuctionSchedulePoint) -> &'static str {
    match value {
        AuctionSchedulePoint::BiddingOpens => "BIDDING_OPENS",
        AuctionSchedulePoint::LiveStarts => "LIVE_STARTS",
        AuctionSchedulePoint::LotsBeginClosing => "LOTS_BEGIN_CLOSING",
        AuctionSchedulePoint::ScheduledEnd => "SCHEDULED_END",
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicAuctionDirectoryData {
    pub items: Vec<PublicAuctionDirectoryItemData>,
    pub page_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_after: Option<DirectoryCursorData>,
}
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicAuctionDirectoryItemData {
    #[schemars(schema_with = "crate::wire::schema::auction_id")]
    pub auction_id: AuctionId,
    pub listing_source: PublicSourceData,
    pub name: Option<crate::values::LocalizedTextData>,
    #[schemars(schema_with = "super::format_schema")]
    pub format: Option<&'static str>,
    pub schedule: super::AuctionScheduleResponseData,
    #[schemars(schema_with = "super::status_schema")]
    pub reported_status: Option<&'static str>,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(schema_with = "crate::wire::schema::datetime")]
    pub created: OffsetDateTime,
}
impl From<auction_service::ports::ListAuctionsDirectoryResult> for PublicAuctionDirectoryData {
    fn from(value: auction_service::ports::ListAuctionsDirectoryResult) -> Self {
        Self {
            page_size: value.cursor.size,
            search_after: value.cursor.search_after.map(|cursor| DirectoryCursorData {
                created: cursor.created,
                auction_id: cursor.auction_id.to_string(),
                scheduled: cursor.scope.sort.map(|_| cursor.scheduled),
                scope: cursor.scope.into(),
            }),
            items: value
                .items
                .into_iter()
                .map(|item| PublicAuctionDirectoryItemData {
                    auction_id: item.auction_id,
                    listing_source: PublicSourceData {
                        listing_source_id: item.source.listing_source_id,
                        name: item.source.name.as_ref().to_owned(),
                        slug_id: item.source.slug_id.to_string(),
                    },
                    name: item.name.map(Into::into),
                    format: item.format.map(AuctionFormat::as_str),
                    schedule: item.schedule.into(),
                    reported_status: item.reported_status.map(AuctionReportedStatus::as_str),
                    created: item.created,
                })
                .collect(),
        }
    }
}

fn schedule_role_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    super::optional_codes([
        schedule_role_code(AuctionSchedulePoint::BiddingOpens),
        schedule_role_code(AuctionSchedulePoint::LiveStarts),
        schedule_role_code(AuctionSchedulePoint::LotsBeginClosing),
        schedule_role_code(AuctionSchedulePoint::ScheduledEnd),
    ])
}
fn sort_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    super::optional_codes(["created", "scheduled"])
}
