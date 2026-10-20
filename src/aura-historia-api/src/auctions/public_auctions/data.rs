use crate::{
    error::{AUCTION_INTERNAL_ERROR, ApiError, BAD_QUERY_PARAMETER_VALUE},
    wire::parse_query_object_id,
};
use application::pagination::Cursor;
use auction_core::{AuctionFormat, AuctionId, AuctionReportedStatus, AuctionSchedulePoint};
use auction_service::{
    ports::{
        AuctionDirectoryCursor, AuctionDirectoryScope, AuctionInstantScheduleFilter,
        ListAuctionsDirectoryRequest, PublicAuctionDetails,
    },
    use_cases::ListAuctionsRequest,
};
use domain_primitives::sort::SortOrder;
use listing_source_core::ListingSourceId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CatalogueQuery {
    #[serde(default, with = "crate::wire::language")]
    pub(super) language: localization::Language,
    #[serde(default, with = "crate::wire::currency")]
    pub(super) currency: money::Currency,
    pub(super) page_size: Option<u64>,
    pub(super) search_after: Option<String>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CatalogueCursorData {
    pub(super) auction_id: String,
    pub(super) catalogue_position: Option<u32>,
    pub(super) product_listing_id: String,
}
impl CatalogueCursorData {
    fn try_into_cursor(
        self,
    ) -> Result<product_listing_service::ports::AuctionCatalogueCursor, ApiError> {
        Ok(product_listing_service::ports::AuctionCatalogueCursor {
            auction_id: parse_query_object_id(
                &self.auction_id,
                "searchAfter.auctionId",
                "Auction",
            )?,
            catalogue_position: self.catalogue_position,
            product_listing_id: parse_query_object_id(
                &self.product_listing_id,
                "searchAfter.productListingId",
                "ProductListing",
            )?,
        })
    }
}

pub(super) fn parse_catalogue_cursor(
    value: String,
) -> Result<product_listing_service::ports::AuctionCatalogueCursor, ApiError> {
    serde_json::from_str::<CatalogueCursorData>(&value)
        .map_err(|error| {
            ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                .with_query_field("searchAfter")
                .with_detail(error.to_string())
        })?
        .try_into_cursor()
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CatalogueData {
    pub(super) items:
        Vec<crate::product_listings::product_data::PersonalizedProductListingDetailsData>,
    pub(super) page_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) search_after: Option<CatalogueCursorData>,
}
impl
    From<
        application::pagination::CursoredResult<
            product_listing_service::use_cases::PersonalizedProductListingDetailsView,
            product_listing_service::ports::AuctionCatalogueCursor,
        >,
    > for CatalogueData
{
    fn from(
        value: application::pagination::CursoredResult<
            product_listing_service::use_cases::PersonalizedProductListingDetailsView,
            product_listing_service::ports::AuctionCatalogueCursor,
        >,
    ) -> Self {
        Self {
            page_size: value.cursor.size,
            search_after: value.cursor.search_after.map(|cursor| CatalogueCursorData {
                auction_id: cursor.auction_id.to_string(),
                catalogue_position: cursor.catalogue_position,
                product_listing_id: cursor.product_listing_id.to_string(),
            }),
            items: value
                .items
                .into_iter()
                .map(crate::product_listings::product_data::personalized_product_details_data)
                .collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DirectoryQuery {
    pub(super) listing_source_id: Option<String>,
    pub(super) format: Option<String>,
    pub(super) reported_status: Option<String>,
    pub(super) time_role: Option<String>,
    pub(super) sort: Option<String>,
    pub(super) order: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub(super) from: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub(super) to: Option<OffsetDateTime>,
    pub(super) page_size: Option<u64>,
    pub(super) search_after: Option<String>,
}

impl DirectoryQuery {
    pub(super) fn try_into_request(self) -> Result<ListAuctionsRequest, ApiError> {
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

fn schedule_point(value: &str) -> Result<AuctionSchedulePoint, ApiError> {
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
) -> Result<Option<AuctionSchedulePoint>, ApiError> {
    match value.unwrap_or("created") {
        "created" => Ok(None),
        "scheduled" => role
            .map(Some)
            .ok_or_else(|| query_error("sort=scheduled requires timeRole.")),
        _ => Err(query_error("sort must be created or scheduled.")),
    }
}

fn parse_directory_order(value: &str) -> Result<SortOrder, ApiError> {
    SortOrder::try_from(value).map_err(|_| query_error("order must be asc or desc."))
}

fn query_error(detail: &'static str) -> ApiError {
    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE).with_detail(detail)
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DirectoryCursorData {
    #[serde(with = "time::serde::rfc3339")]
    pub(super) created: OffsetDateTime,
    pub(super) auction_id: String,
    // The outer Option distinguishes a missing field from a null schedule instant.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_nullable_scheduled",
        deserialize_with = "deserialize_nullable_scheduled"
    )]
    pub(super) scheduled: Option<Option<OffsetDateTime>>,
    pub(super) scope: DirectoryCursorScopeData,
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

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DirectoryCursorScopeData {
    pub(super) listing_source_id: Option<String>,
    pub(super) format: Option<String>,
    pub(super) reported_status: Option<String>,
    pub(super) time_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) sort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) order: Option<SortOrder>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub(super) from: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub(super) to: Option<OffsetDateTime>,
}

impl DirectoryCursorData {
    fn try_into_cursor(self) -> Result<AuctionDirectoryCursor, ApiError> {
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

pub(super) fn parse_directory_cursor(value: String) -> Result<AuctionDirectoryCursor, ApiError> {
    serde_json::from_str::<DirectoryCursorData>(&value)
        .map_err(|error| {
            ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                .with_query_field("searchAfter")
                .with_detail(error.to_string())
        })?
        .try_into_cursor()
}

impl DirectoryCursorScopeData {
    fn try_into_scope(self) -> Result<AuctionDirectoryScope, ApiError> {
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

fn parse_auction_format(value: Option<String>) -> Result<Option<AuctionFormat>, ApiError> {
    value
        .map(|value| {
            value
                .parse()
                .map_err(|_| query_error("format must be LIVE or TIMED."))
        })
        .transpose()
}

fn parse_auction_status(value: Option<String>) -> Result<Option<AuctionReportedStatus>, ApiError> {
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
) -> Result<Option<AuctionInstantScheduleFilter>, ApiError> {
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PublicAuctionData {
    pub(super) auction_id: AuctionId,
    pub(super) listing_source: PublicSourceData,
    pub(super) name: Option<crate::values::LocalizedTextData>,
    pub(super) catalogue_url: Option<url::Url>,
    pub(super) view_url: Option<url::Url>,
    pub(super) format: Option<&'static str>,
    pub(super) schedule: crate::auctions::types::AuctionScheduleResponseData,
    pub(super) reported_status: Option<&'static str>,
    pub(super) reported_lot_count: Option<u32>,
    pub(super) visible_listing_count: u64,
}
impl TryFrom<PublicAuctionDetails> for PublicAuctionData {
    type Error = ApiError;

    fn try_from(value: PublicAuctionDetails) -> Result<Self, Self::Error> {
        let view_url = value
            .catalogue_url
            .as_ref()
            .map(|url| {
                listing_source_core::outbound_url(value.source.referral_configuration.as_ref(), url)
                    .map_err(|_| ApiError::internal_server_error(AUCTION_INTERNAL_ERROR))
            })
            .transpose()?;
        Ok(Self {
            auction_id: value.auction_id,
            listing_source: PublicSourceData {
                listing_source_id: value.source.listing_source_id,
                name: value.source.name.as_ref().to_owned(),
                slug_id: value.source.slug_id.to_string(),
            },
            name: value.name.map(Into::into),
            catalogue_url: value.catalogue_url,
            view_url,
            format: value.format.map(AuctionFormat::as_str),
            schedule: value.schedule.into(),
            reported_status: value.reported_status.map(AuctionReportedStatus::as_str),
            reported_lot_count: value.reported_lot_count.map(|count| count.value()),
            visible_listing_count: value.visible_active_assigned_listing_count,
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PublicSourceData {
    pub(super) listing_source_id: ListingSourceId,
    pub(super) name: String,
    pub(super) slug_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PublicAuctionDirectoryData {
    pub(super) items: Vec<PublicAuctionDirectoryItemData>,
    pub(super) page_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) search_after: Option<DirectoryCursorData>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PublicAuctionDirectoryItemData {
    pub(super) auction_id: AuctionId,
    pub(super) listing_source: PublicSourceData,
    pub(super) name: Option<crate::values::LocalizedTextData>,
    pub(super) format: Option<&'static str>,
    pub(super) schedule: crate::auctions::types::AuctionScheduleResponseData,
    pub(super) reported_status: Option<&'static str>,
    #[serde(with = "time::serde::rfc3339")]
    pub(super) created: OffsetDateTime,
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
