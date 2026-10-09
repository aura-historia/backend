use crate::{
    auctions::types::AuctionAdminData,
    auth::protected_context,
    error::{
        AUCTION_INTERNAL_ERROR, AUCTION_TEMPORARILY_UNAVAILABLE, ApiError,
        BAD_QUERY_PARAMETER_VALUE, BAD_SORT_VALUE, FORBIDDEN, INVALID_CREDENTIALS,
    },
    state::AuctionsState,
    wire::parse_query_object_id,
};
use application::pagination::Cursor;
use auction_core::{AuctionFormat, AuctionReportedStatus, SourceAuctionId};
use auction_service::{
    ports::admin_auction_search_reader::{
        AdminAuctionSearchCursor, AdminAuctionSearchRequest, AdminAuctionSearchScope,
        AdminAuctionSort,
    },
    use_cases::queries::search_admin_auctions::{
        SearchAdminAuctionsError, SearchAdminAuctionsResult,
    },
};
use axum::{
    Json,
    extract::{RawQuery, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use domain_primitives::sort::SortOrder;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

pub async fn search_admin_auctions(
    State(state): State<AuctionsState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let request = serde_qs::from_str::<SearchQuery>(raw_query.as_deref().unwrap_or_default())
        .map_err(|error| query_error(error.to_string()))
        .and_then(SearchQuery::into_request);
    let request = match request {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };
    match state.search.execute(&context, request).await {
        Ok(result) => no_store(Json(SearchResponse::from(result)).into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SearchQuery {
    query: Option<String>,
    listing_source_id: Option<String>,
    source_auction_id: Option<String>,
    format: Option<String>,
    reported_status: Option<String>,
    sort: Option<String>,
    order: Option<String>,
    size: Option<u64>,
    search_after: Option<String>,
}

impl SearchQuery {
    fn into_request(self) -> Result<AdminAuctionSearchRequest, ApiError> {
        let size = self.size.unwrap_or(21).clamp(1, 100);
        if self
            .query
            .as_ref()
            .is_some_and(|query| query.contains('\0'))
        {
            return Err(query_error("query must be NUL-free."));
        }
        let listing_source_id = self
            .listing_source_id
            .map(|value| parse_query_object_id(&value, "listingSourceId", "ListingSource"))
            .transpose()?;
        let source_auction_id = self
            .source_auction_id
            .map(|value| {
                SourceAuctionId::try_from(value).map_err(|_| {
                    query_error(
                        "sourceAuctionId must be nonblank, NUL-free, and at most 512 UTF-8 bytes.",
                    )
                })
            })
            .transpose()?;
        let format = self
            .format
            .map(|value| {
                value
                    .parse::<AuctionFormat>()
                    .map_err(|_| query_error("format must be LIVE or TIMED."))
            })
            .transpose()?;
        let reported_status = self
            .reported_status
            .map(|value| {
                value
                    .parse::<AuctionReportedStatus>()
                    .map_err(|_| query_error("reportedStatus is invalid."))
            })
            .transpose()?;
        let sort = match self.sort.as_deref().unwrap_or("updated") {
            "name" => AdminAuctionSort::Name,
            "created" => AdminAuctionSort::Created,
            "updated" => AdminAuctionSort::Updated,
            _ => {
                return Err(ApiError::bad_request(BAD_SORT_VALUE)
                    .with_query_field("sort")
                    .with_detail("sort must be name, created or updated."));
            }
        };
        let order = SortOrder::try_from(self.order.as_deref().unwrap_or("desc"))
            .map_err(|_| query_error("order must be asc or desc."))?;
        let (sort, order) = if self.sort.is_some() && self.order.is_some() {
            (sort, order)
        } else {
            (AdminAuctionSort::Updated, SortOrder::Desc)
        };
        let scope = AdminAuctionSearchScope {
            query: self.query.filter(|value| !value.is_empty()),
            listing_source_id,
            source_auction_id,
            format,
            reported_status,
            sort,
            order,
        };
        let search_after = self.search_after.map(decode_cursor).transpose()?;
        Ok(AdminAuctionSearchRequest {
            scope,
            cursor: Cursor { size, search_after },
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CursorData {
    version: u8,
    auction_id: String,
    sort_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
    scope: CursorScopeData,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CursorScopeData {
    query: Option<String>,
    listing_source_id: Option<String>,
    source_auction_id: Option<String>,
    format: Option<String>,
    reported_status: Option<String>,
    sort: AdminAuctionSort,
    order: SortOrder,
}

impl From<AdminAuctionSearchScope> for CursorScopeData {
    fn from(scope: AdminAuctionSearchScope) -> Self {
        Self {
            query: scope.query,
            listing_source_id: scope.listing_source_id.map(|id| id.to_string()),
            source_auction_id: scope.source_auction_id.map(Into::into),
            format: scope.format.map(|value| value.as_str().to_owned()),
            reported_status: scope.reported_status.map(|value| value.as_str().to_owned()),
            sort: scope.sort,
            order: scope.order,
        }
    }
}

fn decode_cursor(encoded: String) -> Result<AdminAuctionSearchCursor, ApiError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| query_error("searchAfter is invalid."))?;
    let data: CursorData =
        serde_json::from_slice(&bytes).map_err(|_| query_error("searchAfter is invalid."))?;
    if data.version != 1 {
        return Err(query_error("searchAfter version is invalid."));
    }
    if data
        .sort_name
        .as_ref()
        .is_some_and(|name| name.contains('\0'))
    {
        return Err(query_error("searchAfter name is invalid."));
    }
    let scope = data.scope;
    let source_auction_id = scope
        .source_auction_id
        .map(|raw| {
            let id = SourceAuctionId::try_from(raw.as_str())
                .map_err(|_| query_error("searchAfter scope is invalid."))?;
            if id.as_ref() != raw {
                return Err(query_error("searchAfter scope is invalid."));
            }
            Ok(id)
        })
        .transpose()?;
    Ok(AdminAuctionSearchCursor {
        auction_id: parse_query_object_id(&data.auction_id, "searchAfter.auctionId", "Auction")?,
        sort_name: data.sort_name,
        created: data.created,
        updated: data.updated,
        scope: AdminAuctionSearchScope {
            query: scope.query,
            listing_source_id: scope
                .listing_source_id
                .map(|value| {
                    parse_query_object_id(
                        &value,
                        "searchAfter.scope.listingSourceId",
                        "ListingSource",
                    )
                })
                .transpose()?,
            source_auction_id,
            format: scope
                .format
                .map(|value| {
                    value
                        .parse()
                        .map_err(|_| query_error("searchAfter format is invalid."))
                })
                .transpose()?,
            reported_status: scope
                .reported_status
                .map(|value| {
                    value
                        .parse()
                        .map_err(|_| query_error("searchAfter reportedStatus is invalid."))
                })
                .transpose()?,
            sort: scope.sort,
            order: scope.order,
        },
    })
}

fn encode_cursor(cursor: AdminAuctionSearchCursor) -> String {
    let data = CursorData {
        version: 1,
        auction_id: cursor.auction_id.to_string(),
        sort_name: cursor.sort_name,
        created: cursor.created,
        updated: cursor.updated,
        scope: cursor.scope.into(),
    };
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(&data).expect("valid Auction cursor"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchResponse {
    items: Vec<AuctionAdminData>,
    size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    search_after: Option<String>,
}

impl From<SearchAdminAuctionsResult> for SearchResponse {
    fn from(result: SearchAdminAuctionsResult) -> Self {
        let size = result.items.len();
        Self {
            items: result
                .items
                .into_iter()
                .map(AuctionAdminData::from)
                .collect(),
            size,
            search_after: result.cursor.search_after.map(encode_cursor),
        }
    }
}

impl From<SearchAdminAuctionsError> for ApiError {
    fn from(error: SearchAdminAuctionsError) -> Self {
        match error {
            SearchAdminAuctionsError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
            }
            SearchAdminAuctionsError::Forbidden => ApiError::forbidden(FORBIDDEN),
            SearchAdminAuctionsError::CursorScopeMismatch
            | SearchAdminAuctionsError::InvalidPageSize => {
                ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            }
            SearchAdminAuctionsError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(AUCTION_TEMPORARILY_UNAVAILABLE)
            }
            SearchAdminAuctionsError::InvalidReadModel { .. }
            | SearchAdminAuctionsError::Internal { .. } => {
                ApiError::internal_server_error(AUCTION_INTERNAL_ERROR)
            }
        }
    }
}

fn query_error(detail: impl Into<String>) -> ApiError {
    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE).with_detail(detail.into())
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use auction_core::AuctionId;
    use listing_source_core::ListingSourceId;

    fn parse(raw: &str) -> Result<AdminAuctionSearchRequest, ApiError> {
        serde_qs::from_str::<SearchQuery>(raw)
            .map_err(|error| query_error(error.to_string()))?
            .into_request()
    }

    #[test]
    fn parses_filters_and_defaults_and_trims_exact_source_key() {
        let source_id = ListingSourceId::new();
        let request = parse(&format!("query=SALE&listingSourceId={source_id}&sourceAuctionId=%20Sale-42%20&format=LIVE&reportedStatus=ENDED"))
            .expect("valid search");
        assert_eq!(Some("SALE"), request.scope.query.as_deref());
        assert_eq!(Some(source_id), request.scope.listing_source_id);
        assert_eq!(
            Some("Sale-42"),
            request.scope.source_auction_id.as_ref().map(AsRef::as_ref)
        );
        assert_eq!(AdminAuctionSort::Updated, request.scope.sort);
        assert_eq!(SortOrder::Desc, request.scope.order);
        assert_eq!(21, request.cursor.size);
        assert_eq!(1, parse("size=0").expect("clamped").cursor.size);
        assert_eq!(100, parse("size=101").expect("clamped").cursor.size);
    }

    #[test]
    fn decodes_form_encoded_spaces_and_literal_pluses() {
        let request = parse("query=Hidden+Auction&sourceAuctionId=+conflict+%2F+42+")
            .expect("valid form-encoded search");
        assert_eq!(Some("Hidden Auction"), request.scope.query.as_deref());
        assert_eq!(
            Some("conflict / 42"),
            request.scope.source_auction_id.as_ref().map(AsRef::as_ref)
        );
        assert_eq!(
            Some("Sale+42"),
            parse("query=Sale%2B42")
                .expect("literal plus")
                .scope
                .query
                .as_deref()
        );
    }

    #[test]
    fn rejects_bad_filters_and_page_sizes() {
        for raw in [
            "size=-1",
            "size=not-a-number",
            "format=live",
            "reportedStatus=unknown",
            "sort=random",
            "order=ascending",
            "sourceAuctionId=%20%20",
            "query=bad%00text",
            "searchAfter=broken",
            "other=field",
        ] {
            assert!(parse(raw).is_err(), "accepted {raw}");
        }
    }

    #[test]
    fn opaque_cursor_round_trips_and_rejects_changed_scope() {
        let mut request = parse("query=sale&sort=name&order=asc&size=1").expect("valid search");
        let cursor = AdminAuctionSearchCursor {
            auction_id: AuctionId::new(),
            sort_name: Some("sale".into()),
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
            scope: request.scope.clone(),
        };
        let encoded = encode_cursor(cursor.clone());
        assert_eq!(cursor, decode_cursor(encoded.clone()).expect("round trip"));
        let mut invalid = cursor.clone();
        invalid.sort_name = Some("bad\0name".into());
        assert!(decode_cursor(encode_cursor(invalid)).is_err());
        request.cursor.search_after = Some(decode_cursor(encoded).expect("cursor"));
        assert_eq!(
            request
                .cursor
                .search_after
                .as_ref()
                .map(|after| &after.scope),
            Some(&request.scope)
        );
        let mut changed = request.scope.clone();
        changed.query = Some("other".into());
        assert_ne!(changed, request.cursor.search_after.expect("cursor").scope);
    }
}
