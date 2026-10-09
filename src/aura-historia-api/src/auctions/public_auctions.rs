use crate::{
    auth::{OptionalAuthExtractor, request_metadata},
    error::{
        AUCTION_INTERNAL_ERROR, AUCTION_NOT_FOUND, AUCTION_TEMPORARILY_UNAVAILABLE, ApiError,
        BAD_QUERY_PARAMETER_VALUE,
    },
    state::PublicAuctionsState,
    wire::{parse_path_object_id, parse_query_object_id},
};
use application::pagination::Cursor;
use auction_core::{AuctionFormat, AuctionId, AuctionReportedStatus, AuctionSchedulePoint};
use auction_service::{
    ports::{
        AuctionDirectoryCursor, AuctionDirectoryScope, AuctionInstantScheduleFilter,
        ListAuctionsDirectoryRequest, PublicAuctionDetails,
    },
    use_cases::{GetPublicAuctionError, ListAuctionsError, ListAuctionsRequest},
};
use axum::{
    Json,
    extract::{Path, RawQuery, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use domain_primitives::sort::SortOrder;
use listing_source_core::ListingSourceId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

pub async fn get_public_auction(
    State(state): State<PublicAuctionsState>,
    headers: HeaderMap,
    Path(raw_auction_id): Path<String>,
) -> Response {
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return no_store(ApiError::from(error).into_response()),
    };
    let auction_id =
        match parse_path_object_id::<AuctionId>(&raw_auction_id, "auctionId", "Auction") {
            Ok(value) => value,
            Err(error) => return no_store(error.into_response()),
        };
    match state
        .get
        .execute(&principal.operation_context(metadata), auction_id)
        .await
    {
        Ok(view) => match PublicAuctionData::try_from(view) {
            Ok(data) => crate::transport::cache::anonymous_shared_success(
                Json(data).into_response(),
                &headers,
                matches!(&principal, crate::auth::TransportPrincipal::Anonymous),
                60,
            ),
            Err(error) => no_store(error.into_response()),
        },
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

pub async fn get_auction_catalogue(
    State(state): State<PublicAuctionsState>,
    headers: HeaderMap,
    Path(raw_auction_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let auction_id =
        match parse_path_object_id::<AuctionId>(&raw_auction_id, "auctionId", "Auction") {
            Ok(value) => value,
            Err(error) => return no_store(error.into_response()),
        };
    let query = match serde_qs::from_str::<CatalogueQuery>(raw_query.as_deref().unwrap_or_default())
    {
        Ok(value) => value,
        Err(error) => {
            return no_store(
                ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                    .with_detail(error.to_string())
                    .into_response(),
            );
        }
    };
    let cursor = match query.search_after.map(parse_catalogue_cursor).transpose() {
        Ok(value) => application::pagination::Cursor {
            size: query.page_size.unwrap_or(21),
            search_after: value,
        },
        Err(error) => return no_store(error.into_response()),
    };
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(value) => value,
        Err(error) => return no_store(ApiError::from(error).into_response()),
    };
    match state
        .catalogue
        .execute(
            &principal.operation_context(metadata),
            product_listing_service::use_cases::GetAuctionCatalogueRequest {
                auction_id,
                language: query.language,
                currency: query.currency,
                cursor: Some(cursor),
            },
        )
        .await
    {
        Ok(page) => crate::transport::cache::anonymous_shared_success(
            Json(CatalogueData::from(page)).into_response(),
            &headers,
            matches!(&principal, crate::auth::TransportPrincipal::Anonymous),
            60,
        ),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

pub async fn list_public_auctions(
    State(state): State<PublicAuctionsState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let data = match serde_qs::from_str::<DirectoryQuery>(raw_query.as_deref().unwrap_or_default())
    {
        Ok(value) => value,
        Err(error) => {
            return no_store(
                ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                    .with_detail(error.to_string())
                    .into_response(),
            );
        }
    };
    let request = match data.try_into_request() {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return no_store(ApiError::from(error).into_response()),
    };
    match state
        .list
        .execute(&principal.operation_context(metadata), request)
        .await
    {
        Ok(result) => crate::transport::cache::anonymous_shared_success(
            Json(PublicAuctionDirectoryData::from(result)).into_response(),
            &headers,
            matches!(&principal, crate::auth::TransportPrincipal::Anonymous),
            60,
        ),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogueQuery {
    #[serde(default, with = "crate::wire::language")]
    language: localization::Language,
    #[serde(default, with = "crate::wire::currency")]
    currency: money::Currency,
    page_size: Option<u64>,
    search_after: Option<String>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogueCursorData {
    auction_id: String,
    catalogue_position: Option<u32>,
    product_listing_id: String,
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

fn parse_catalogue_cursor(
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
struct CatalogueData {
    items: Vec<crate::product_listings::product_data::PersonalizedProductListingDetailsData>,
    page_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    search_after: Option<CatalogueCursorData>,
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
struct DirectoryQuery {
    listing_source_id: Option<String>,
    format: Option<String>,
    reported_status: Option<String>,
    time_role: Option<String>,
    sort: Option<String>,
    order: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    from: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    to: Option<OffsetDateTime>,
    page_size: Option<u64>,
    search_after: Option<String>,
}

impl DirectoryQuery {
    fn try_into_request(self) -> Result<ListAuctionsRequest, ApiError> {
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
struct DirectoryCursorData {
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    auction_id: String,
    // The outer Option distinguishes a missing field from a null schedule instant.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_nullable_scheduled",
        deserialize_with = "deserialize_nullable_scheduled"
    )]
    scheduled: Option<Option<OffsetDateTime>>,
    scope: DirectoryCursorScopeData,
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
struct DirectoryCursorScopeData {
    listing_source_id: Option<String>,
    format: Option<String>,
    reported_status: Option<String>,
    time_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    order: Option<SortOrder>,
    #[serde(with = "time::serde::rfc3339::option")]
    from: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    to: Option<OffsetDateTime>,
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

fn parse_directory_cursor(value: String) -> Result<AuctionDirectoryCursor, ApiError> {
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
struct PublicAuctionData {
    auction_id: AuctionId,
    listing_source: PublicSourceData,
    name: Option<crate::values::LocalizedTextData>,
    catalogue_url: Option<url::Url>,
    view_url: Option<url::Url>,
    format: Option<&'static str>,
    schedule: crate::auctions::types::AuctionScheduleResponseData,
    reported_status: Option<&'static str>,
    reported_lot_count: Option<u32>,
    visible_listing_count: u64,
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
struct PublicSourceData {
    listing_source_id: ListingSourceId,
    name: String,
    slug_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicAuctionDirectoryData {
    items: Vec<PublicAuctionDirectoryItemData>,
    page_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    search_after: Option<DirectoryCursorData>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicAuctionDirectoryItemData {
    auction_id: AuctionId,
    listing_source: PublicSourceData,
    name: Option<crate::values::LocalizedTextData>,
    format: Option<&'static str>,
    schedule: crate::auctions::types::AuctionScheduleResponseData,
    reported_status: Option<&'static str>,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
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

impl From<GetPublicAuctionError> for ApiError {
    fn from(error: GetPublicAuctionError) -> Self {
        match error {
            GetPublicAuctionError::NotFound => ApiError::not_found(AUCTION_NOT_FOUND),
            GetPublicAuctionError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(AUCTION_TEMPORARILY_UNAVAILABLE)
            }
            GetPublicAuctionError::InvalidReadModel { .. } => {
                ApiError::internal_server_error(AUCTION_INTERNAL_ERROR)
            }
        }
    }
}
impl From<product_listing_service::use_cases::GetAuctionCatalogueError> for ApiError {
    fn from(error: product_listing_service::use_cases::GetAuctionCatalogueError) -> Self {
        match error {
            product_listing_service::use_cases::GetAuctionCatalogueError::AuctionNotFound => {
                ApiError::not_found(AUCTION_NOT_FOUND)
            }
            product_listing_service::use_cases::GetAuctionCatalogueError::CursorScopeMismatch => {
                ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            }
            product_listing_service::use_cases::GetAuctionCatalogueError::TemporarilyUnavailable { .. }
            | product_listing_service::use_cases::GetAuctionCatalogueError::PricingFxSnapshotMissing => {
                ApiError::service_unavailable(AUCTION_TEMPORARILY_UNAVAILABLE)
            }
            product_listing_service::use_cases::GetAuctionCatalogueError::InvalidReadModel { .. }
            | product_listing_service::use_cases::GetAuctionCatalogueError::PricingPresentationFailed { .. } => {
                ApiError::internal_server_error(AUCTION_INTERNAL_ERROR)
            }
        }
    }
}
impl From<ListAuctionsError> for ApiError {
    fn from(error: ListAuctionsError) -> Self {
        match error {
            ListAuctionsError::IncompleteScheduleInstantRange
            | ListAuctionsError::ScheduledSortRoleMismatch
            | ListAuctionsError::InvalidScheduleInstantRange
            | ListAuctionsError::CursorScopeMismatch => {
                ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            }
            ListAuctionsError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(AUCTION_TEMPORARILY_UNAVAILABLE)
            }
            ListAuctionsError::InvalidReadModel { .. } => {
                ApiError::internal_server_error(AUCTION_INTERNAL_ERROR)
            }
        }
    }
}
fn no_store(response: Response) -> Response {
    crate::transport::cache::private_no_store(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use application::{operation_context::OperationContext, pagination::CursoredResult};
    use auction_service::{
        ports::PublicAuctionSourceSummary,
        use_cases::{
            GetPublicAuctionResult, GetPublicAuctionUseCase, ListAuctionsResult,
            ListAuctionsUseCase,
        },
    };
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode, header},
        routing::get,
    };
    use money::Currency;
    use std::sync::{Arc, Mutex, MutexGuard};
    use tower::ServiceExt;
    use user_core::user_id::UserId;

    #[derive(Clone)]
    struct FakeGet {
        result: GetPublicAuctionResult,
    }

    #[async_trait::async_trait]
    impl GetPublicAuctionUseCase for FakeGet {
        async fn execute(
            &self,
            _context: &OperationContext,
            _auction_id: AuctionId,
        ) -> Result<GetPublicAuctionResult, GetPublicAuctionError> {
            Ok(self.result.clone())
        }
    }

    #[derive(Clone, Copy)]
    struct FakeList;

    #[async_trait::async_trait]
    impl ListAuctionsUseCase for FakeList {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: ListAuctionsRequest,
        ) -> Result<ListAuctionsResult, ListAuctionsError> {
            Ok(CursoredResult::default())
        }
    }

    #[derive(Clone, Default)]
    struct FakeCatalogue {
        requests: Arc<Mutex<Vec<product_listing_service::use_cases::GetAuctionCatalogueRequest>>>,
    }

    #[async_trait::async_trait]
    impl product_listing_service::use_cases::GetAuctionCatalogueUseCase for FakeCatalogue {
        async fn execute(
            &self,
            _context: &OperationContext,
            request: product_listing_service::use_cases::GetAuctionCatalogueRequest,
        ) -> Result<
            CursoredResult<
                product_listing_service::use_cases::PersonalizedProductListingDetailsView,
                product_listing_service::ports::AuctionCatalogueCursor,
            >,
            product_listing_service::use_cases::GetAuctionCatalogueError,
        > {
            lock(&self.requests).push(request);
            Ok(CursoredResult::default())
        }
    }

    #[derive(Clone, Copy)]
    struct FakeAuthenticator {
        reject: bool,
    }

    #[async_trait::async_trait]
    impl TokenAuthenticator for FakeAuthenticator {
        async fn authenticate(
            &self,
            _bearer_token: &str,
            _metadata: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            if self.reject {
                Err(AuthError::InvalidCredentials)
            } else {
                Ok(TransportPrincipal::User {
                    user_id: UserId::new(),
                    auth_method: AuthMethod::CognitoJwt,
                    capabilities: Default::default(),
                })
            }
        }
    }

    fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
        match value.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn detail() -> PublicAuctionDetails {
        PublicAuctionDetails {
            auction_id: AuctionId::new(),
            source: PublicAuctionSourceSummary {
                listing_source_id: ListingSourceId::new(),
                slug_id: listing_source_core::ListingSourceSlugId::raw("source")
                    .unwrap_or_else(|error| panic!("valid source slug: {error}")),
                name: listing_source_core::ListingSourceName::try_from("Source")
                    .unwrap_or_else(|error| panic!("valid source name: {error}")),
                referral_configuration: None,
            },
            name: None,
            catalogue_url: None,
            format: None,
            schedule: auction_core::AuctionSchedule::default(),
            reported_status: None,
            reported_lot_count: None,
            visible_active_assigned_listing_count: 0,
        }
    }

    fn app(catalogue: FakeCatalogue) -> Router {
        app_with_auth(catalogue, FakeAuthenticator { reject: true })
    }

    fn app_with_auth(catalogue: FakeCatalogue, authenticator: FakeAuthenticator) -> Router {
        Router::new()
            .route("/api/v1/auctions", get(list_public_auctions))
            .route("/api/v1/auctions/{auction_id}", get(get_public_auction))
            .route(
                "/api/v1/auctions/{auction_id}/product-listings",
                get(get_auction_catalogue),
            )
            .with_state(PublicAuctionsState::new(
                Arc::new(FakeGet { result: detail() }),
                Arc::new(FakeList),
                Arc::new(catalogue),
                Arc::new(authenticator),
            ))
    }

    #[tokio::test]
    async fn should_return_safe_public_auction_detail_with_shared_cache_for_anonymous_request()
    -> Result<(), Box<dyn std::error::Error>> {
        let auction_id = detail().auction_id;
        let response = app(FakeCatalogue::default())
            .oneshot(Request::get(format!("/api/v1/auctions/{auction_id}")).body(Body::empty())?)
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            "public, max-age=0, s-maxage=60, stale-if-error=0",
            response.headers()[header::CACHE_CONTROL]
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let body: serde_json::Value = serde_json::from_slice(&body)?;
        assert!(body.get("sourceAuctionId").is_none());
        assert!(body.get("description").is_none());
        assert!(body.get("evidence").is_none());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_public_auction_id_without_store()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = app(FakeCatalogue::default())
            .oneshot(Request::get("/api/v1/auctions/not-an-auction").body(Body::empty())?)
            .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!(
            "private, no-store",
            response.headers()[header::CACHE_CONTROL]
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_map_catalogue_currency_and_page_size_for_anonymous_request()
    -> Result<(), Box<dyn std::error::Error>> {
        let catalogue = FakeCatalogue::default();
        let auction_id = detail().auction_id;
        let response = app(catalogue.clone())
            .oneshot(
                Request::get(format!(
                    "/api/v1/auctions/{auction_id}/product-listings?currency=USD&pageSize=25"
                ))
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            "public, max-age=0, s-maxage=60, stale-if-error=0",
            response.headers()[header::CACHE_CONTROL]
        );
        assert!(matches!(
            lock(&catalogue.requests).as_slice(),
            [request] if request.auction_id == auction_id
                && request.currency == Currency::Usd
                && request.cursor.as_ref().map(|cursor| cursor.size) == Some(25)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_an_invalid_optional_public_credential()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = app(FakeCatalogue::default())
            .oneshot(
                Request::get("/api/v1/auctions")
                    .header(header::AUTHORIZATION, "Bearer invalid")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::UNAUTHORIZED, response.status());
        assert_eq!(
            "private, no-store",
            response.headers()[header::CACHE_CONTROL]
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_return_private_no_store_for_valid_credential_on_detail_and_catalogue()
    -> Result<(), Box<dyn std::error::Error>> {
        let auction_id = detail().auction_id;
        let authenticator = FakeAuthenticator { reject: false };

        let detail_response = app_with_auth(FakeCatalogue::default(), authenticator)
            .oneshot(
                Request::get(format!("/api/v1/auctions/{auction_id}"))
                    .header(header::AUTHORIZATION, "Bearer valid")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(StatusCode::OK, detail_response.status());
        assert_eq!(
            "private, no-store",
            detail_response.headers()[header::CACHE_CONTROL]
        );

        let catalogue_response = app_with_auth(FakeCatalogue::default(), authenticator)
            .oneshot(
                Request::get(format!("/api/v1/auctions/{auction_id}/product-listings"))
                    .header(header::AUTHORIZATION, "Bearer valid")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(StatusCode::OK, catalogue_response.status());
        assert_eq!(
            "private, no-store",
            catalogue_response.headers()[header::CACHE_CONTROL]
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_cache_anonymous_auction_directory_for_sixty_seconds()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = app(FakeCatalogue::default())
            .oneshot(Request::get("/api/v1/auctions").body(Body::empty())?)
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            "public, max-age=0, s-maxage=60, stale-if-error=0",
            response.headers()[header::CACHE_CONTROL]
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_directory_sort_parameters_without_store()
    -> Result<(), Box<dyn std::error::Error>> {
        for query in [
            "sort=scheduled",
            "sort=bogus",
            "order=bogus",
            "sort=scheduled&timeRole=LIVE_STARTS&to=2026-01-01T00%3A00%3A00Z",
        ] {
            let response = app(FakeCatalogue::default())
                .oneshot(Request::get(format!("/api/v1/auctions?{query}")).body(Body::empty())?)
                .await?;
            assert_eq!(StatusCode::BAD_REQUEST, response.status(), "{query}");
            assert_eq!(
                "private, no-store",
                response.headers()[header::CACHE_CONTROL]
            );
        }
        Ok(())
    }

    #[test]
    fn should_parse_catalogue_cursor_as_complete_json_string()
    -> Result<(), Box<dyn std::error::Error>> {
        let auction_id = AuctionId::new();
        let product_listing_id = product_listing_core::product_listing_id::ProductListingId::new();
        let value = serde_json::json!({
            "auctionId": auction_id.to_string(),
            "cataloguePosition": 7,
            "productListingId": product_listing_id.to_string(),
        });

        let cursor = parse_catalogue_cursor(value.to_string())?;

        assert_eq!(auction_id, cursor.auction_id);
        assert_eq!(Some(7), cursor.catalogue_position);
        assert_eq!(product_listing_id, cursor.product_listing_id);
        Ok(())
    }

    #[test]
    fn should_parse_directory_cursor_as_complete_json_string()
    -> Result<(), Box<dyn std::error::Error>> {
        let auction_id = AuctionId::new();
        let value = serde_json::json!({
            "created": "1970-01-01T00:00:00Z",
            "auctionId": auction_id.to_string(),
            "scope": {
                "listingSourceId": null,
                "format": null,
                "reportedStatus": null,
                "timeRole": null,
                "from": null,
                "to": null,
            },
        });

        let cursor = parse_directory_cursor(value.to_string())?;

        assert_eq!(auction_id, cursor.auction_id);
        assert_eq!(None, cursor.scope.listing_source_id);
        assert_eq!(None, cursor.scope.sort);
        assert_eq!(None, cursor.scheduled);
        Ok(())
    }

    #[test]
    fn should_round_trip_returned_cursor_through_url_encoded_json()
    -> Result<(), Box<dyn std::error::Error>> {
        let auction_id = AuctionId::new();
        let product_listing_id = product_listing_core::product_listing_id::ProductListingId::new();
        let catalogue_json = serde_json::to_string(&CatalogueCursorData {
            auction_id: auction_id.to_string(),
            catalogue_position: Some(7),
            product_listing_id: product_listing_id.to_string(),
        })?;
        let catalogue_query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("searchAfter", &catalogue_json)
            .finish();
        let catalogue_query = serde_qs::from_str::<CatalogueQuery>(&catalogue_query)?;
        let catalogue_cursor = parse_catalogue_cursor(
            catalogue_query
                .search_after
                .ok_or("catalogue cursor missing after URL decoding")?,
        )?;
        assert_eq!(auction_id, catalogue_cursor.auction_id);
        assert_eq!(Some(7), catalogue_cursor.catalogue_position);

        let directory_json = serde_json::to_string(&DirectoryCursorData {
            created: OffsetDateTime::UNIX_EPOCH,
            auction_id: auction_id.to_string(),
            scheduled: None,
            scope: DirectoryCursorScopeData {
                listing_source_id: None,
                format: None,
                reported_status: None,
                time_role: None,
                sort: None,
                order: None,
                from: None,
                to: None,
            },
        })?;
        let directory_query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("searchAfter", &directory_json)
            .finish();
        let directory_query = serde_qs::from_str::<DirectoryQuery>(&directory_query)?;
        let directory_cursor = parse_directory_cursor(
            directory_query
                .search_after
                .ok_or("directory cursor missing after URL decoding")?,
        )?;
        assert_eq!(auction_id, directory_cursor.auction_id);
        assert_eq!(OffsetDateTime::UNIX_EPOCH, directory_cursor.created);
        Ok(())
    }

    #[test]
    fn should_parse_directory_sort_order_and_unbounded_schedule()
    -> Result<(), Box<dyn std::error::Error>> {
        let default = serde_qs::from_str::<DirectoryQuery>("")?.try_into_request()?;
        assert_eq!(None, default.sort);
        assert_eq!(SortOrder::Desc, default.order());
        assert_eq!(None, default.scope().order);

        let scheduled =
            serde_qs::from_str::<DirectoryQuery>("sort=scheduled&timeRole=LIVE_STARTS")?
                .try_into_request()?;
        assert_eq!(Some(AuctionSchedulePoint::LiveStarts), scheduled.sort);
        assert_eq!(SortOrder::Asc, scheduled.order());
        assert_eq!(None, scheduled.schedule);
        assert_eq!(Some(SortOrder::Asc), scheduled.scope().order);

        let descending = serde_qs::from_str::<DirectoryQuery>(
            "sort=scheduled&timeRole=SCHEDULED_END&order=desc",
        )?
        .try_into_request()?;
        assert_eq!(SortOrder::Desc, descending.order());
        let ascending_created =
            serde_qs::from_str::<DirectoryQuery>("sort=created&order=asc")?.try_into_request()?;
        assert_eq!(SortOrder::Asc, ascending_created.order());
        Ok(())
    }

    #[test]
    fn should_reject_invalid_directory_sort_and_partial_schedule_ranges() {
        for query in [
            "sort=scheduled",
            "sort=unexpected",
            "order=up",
            "timeRole=LIVE_STARTS",
            "sort=scheduled&timeRole=LIVE_STARTS&from=2026-01-01T00%3A00%3A00Z",
            "from=2026-01-01T00%3A00%3A00Z",
        ] {
            let parsed = serde_qs::from_str::<DirectoryQuery>(query)
                .unwrap_or_else(|error| panic!("failed to parse {query}: {error}"));
            assert!(parsed.try_into_request().is_err(), "accepted {query}");
        }
    }

    #[test]
    fn should_round_trip_nullable_scheduled_cursor_and_reject_missing_instant()
    -> Result<(), Box<dyn std::error::Error>> {
        let auction_id = AuctionId::new();
        let cursor = DirectoryCursorData {
            created: OffsetDateTime::UNIX_EPOCH,
            auction_id: auction_id.to_string(),
            scheduled: Some(None),
            scope: DirectoryCursorScopeData {
                listing_source_id: None,
                format: None,
                reported_status: None,
                time_role: Some("LIVE_STARTS".to_owned()),
                sort: Some("scheduled".to_owned()),
                order: Some(SortOrder::Asc),
                from: None,
                to: None,
            },
        };
        let value = serde_json::to_value(&cursor)?;
        assert!(
            value
                .get("scheduled")
                .is_some_and(serde_json::Value::is_null)
        );
        let parsed = parse_directory_cursor(value.to_string())?;
        assert_eq!(None, parsed.scheduled);
        assert_eq!(Some(AuctionSchedulePoint::LiveStarts), parsed.scope.sort);
        assert!(
            parse_directory_cursor(
                serde_json::json!({
                    "created": "1970-01-01T00:00:00Z", "auctionId": auction_id.to_string(),
                    "scope": value["scope"]
                })
                .to_string()
            )
            .is_err()
        );
        let instant = time::macros::datetime!(2026-01-01 00:00 UTC);
        let mut cursor = cursor;
        cursor.scheduled = Some(Some(instant));
        assert_eq!(
            Some(instant),
            parse_directory_cursor(serde_json::to_string(&cursor)?)?.scheduled
        );
        Ok(())
    }

    #[test]
    fn should_keep_legacy_directory_cursor_json_shape() -> Result<(), Box<dyn std::error::Error>> {
        let cursor = DirectoryCursorData {
            created: OffsetDateTime::UNIX_EPOCH,
            auction_id: AuctionId::new().to_string(),
            scheduled: None,
            scope: DirectoryCursorScopeData {
                listing_source_id: None,
                format: None,
                reported_status: None,
                time_role: None,
                sort: None,
                order: None,
                from: None,
                to: None,
            },
        };
        let value = serde_json::to_value(cursor)?;
        assert!(value.get("scheduled").is_none());
        assert!(value["scope"].get("sort").is_none());
        assert!(value["scope"].get("order").is_none());
        Ok(())
    }

    #[test]
    fn should_reject_malformed_json_cursors() {
        assert!(parse_catalogue_cursor("not-json".to_owned()).is_err());
        assert!(parse_directory_cursor("not-json".to_owned()).is_err());
    }
}
