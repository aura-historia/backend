use super::types::SummaryData;
use crate::{
    error::{ApiError, BAD_QUERY_PARAMETER_VALUE},
    wire::parse_query_object_id,
};
use application::pagination::Cursor;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use domain_primitives::sort::SortOrder;
use product_listing_core::{
    listing_availability::ListingAvailability, listing_lifecycle::ListingLifecycle,
    source_listing_id::SourceListingId,
};
use product_listing_service::ports::admin_product_listing_reader::*;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SearchQuery {
    query: Option<String>,
    listing_source_id: Option<String>,
    source_listing_id: Option<String>,
    lifecycle: Option<String>,
    availability: Option<String>,
    auction_id: Option<String>,
    sort: Option<String>,
    order: Option<String>,
    size: Option<u64>,
    search_after: Option<String>,
}
impl SearchQuery {
    pub(crate) fn into_request(self) -> Result<AdminProductListingSearchRequest, ApiError> {
        let scope = ScopeData {
            query: self.query,
            listing_source_id: self.listing_source_id,
            source_listing_id: self.source_listing_id,
            lifecycle: self.lifecycle,
            availability: self.availability,
            auction_id: self.auction_id,
            sort: self.sort.unwrap_or_else(|| "updated".to_owned()),
            order: self.order.unwrap_or_else(|| "desc".to_owned()),
        }
        .into_scope()?;
        let search_after = self.search_after.map(decode_cursor).transpose()?;
        Ok(AdminProductListingSearchRequest {
            scope,
            cursor: Cursor {
                size: self.size.unwrap_or(21).clamp(1, 100),
                search_after,
            },
        })
    }
    pub(crate) fn into_partner_request(
        mut self,
        source: listing_source_core::ListingSourceId,
    ) -> Result<AdminProductListingSearchRequest, ApiError> {
        if self.listing_source_id.is_some() {
            return Err(query_error(
                "listingSourceId",
                "The ListingSource is specified by the path.",
            ));
        }
        self.listing_source_id = Some(source.to_string());
        self.into_request()
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScopeData {
    query: Option<String>,
    listing_source_id: Option<String>,
    source_listing_id: Option<String>,
    lifecycle: Option<String>,
    availability: Option<String>,
    auction_id: Option<String>,
    sort: String,
    order: String,
}
impl ScopeData {
    fn into_scope(self) -> Result<AdminProductListingSearchScope, ApiError> {
        if self
            .query
            .as_ref()
            .is_some_and(|q| q.contains('\0') || q.len() > 512)
        {
            return Err(query_error(
                "query",
                "query must be NUL-free and at most 512 UTF-8 bytes.",
            ));
        }
        Ok(AdminProductListingSearchScope {
            query: self
                .query
                .map(|value| value.trim().to_lowercase())
                .filter(|value| !value.is_empty()),
            listing_source_id: self
                .listing_source_id
                .map(|id| parse_query_object_id(&id, "listingSourceId", "ListingSource"))
                .transpose()?,
            source_listing_id: self
                .source_listing_id
                .map(|id| {
                    SourceListingId::try_from(id).map_err(|_| {
                        query_error("sourceListingId", "Invalid source-local identity.")
                    })
                })
                .transpose()?,
            lifecycle: self
                .lifecycle
                .map(|value| {
                    ListingLifecycle::from_code(&value).ok_or_else(|| {
                        query_error("lifecycle", "lifecycle must be ACTIVE or WITHDRAWN.")
                    })
                })
                .transpose()?,
            availability: self
                .availability
                .map(|value| {
                    ListingAvailability::from_code(&value)
                        .ok_or_else(|| query_error("availability", "Invalid availability."))
                })
                .transpose()?,
            auction_id: self
                .auction_id
                .map(|id| parse_query_object_id(&id, "auctionId", "Auction"))
                .transpose()?,
            sort: match self.sort.as_str() {
                "created" => AdminProductListingSort::Created,
                "updated" => AdminProductListingSort::Updated,
                _ => return Err(query_error("sort", "sort must be created or updated.")),
            },
            order: SortOrder::try_from(self.order.as_str())
                .map_err(|_| query_error("order", "order must be asc or desc."))?,
        })
    }
}
impl From<AdminProductListingSearchScope> for ScopeData {
    fn from(value: AdminProductListingSearchScope) -> Self {
        Self {
            query: value.query,
            listing_source_id: value.listing_source_id.map(|id| id.to_string()),
            source_listing_id: value.source_listing_id.map(Into::into),
            lifecycle: value.lifecycle.map(|v| v.as_str().to_owned()),
            availability: value.availability.map(|v| v.as_str().to_owned()),
            auction_id: value.auction_id.map(|id| id.to_string()),
            sort: match value.sort {
                AdminProductListingSort::Created => "created",
                AdminProductListingSort::Updated => "updated",
            }
            .to_owned(),
            order: match value.order {
                SortOrder::Asc => "asc",
                SortOrder::Desc => "desc",
            }
            .to_owned(),
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CursorData {
    version: u8,
    product_listing_id: String,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
    scope: ScopeData,
}
fn decode_cursor(encoded: String) -> Result<AdminProductListingSearchCursor, ApiError> {
    if encoded.len() > 8192 {
        return Err(query_error("searchAfter", "Invalid cursor."));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| query_error("searchAfter", "Invalid cursor."))?;
    let data: CursorData = serde_json::from_slice(&bytes)
        .map_err(|_| query_error("searchAfter", "Invalid cursor."))?;
    if data.version != 1 {
        return Err(query_error("searchAfter", "Invalid cursor version."));
    }
    Ok(AdminProductListingSearchCursor {
        product_listing_id: parse_query_object_id(
            &data.product_listing_id,
            "searchAfter",
            "ProductListing",
        )?,
        created: data.created,
        updated: data.updated,
        scope: data
            .scope
            .into_scope()
            .map_err(|_| query_error("searchAfter", "Invalid cursor scope."))?,
    })
}
fn encode_cursor(value: AdminProductListingSearchCursor) -> String {
    URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&CursorData {
            version: 1,
            product_listing_id: value.product_listing_id.to_string(),
            created: value.created,
            updated: value.updated,
            scope: value.scope.into(),
        })
        .expect("valid listing cursor"),
    )
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchResponse {
    items: Vec<SummaryData>,
    size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    search_after: Option<String>,
}
impl From<AdminProductListingSearchResult> for SearchResponse {
    fn from(value: AdminProductListingSearchResult) -> Self {
        Self {
            size: value.items.len(),
            items: value.items.into_iter().map(Into::into).collect(),
            search_after: value.cursor.search_after.map(encode_cursor),
        }
    }
}
fn query_error(field: &'static str, detail: &str) -> ApiError {
    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
        .with_query_field(field)
        .with_detail(detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use product_listing_core::product_listing_id::ProductListingId;

    fn parse(raw: &str) -> Result<AdminProductListingSearchRequest, ApiError> {
        serde_qs::from_str::<SearchQuery>(raw)
            .map_err(|_| query_error("query", "Invalid query."))?
            .into_request()
    }
    #[test]
    fn directory_defaults_and_scope_roundtrip() {
        let request =
            parse("query=%20VASE%20&sourceListingId=%20SKU-42%20&size=0&order=asc").unwrap();
        assert_eq!(1, request.cursor.size);
        assert_eq!(Some("vase"), request.scope.query.as_deref());
        assert_eq!(
            Some("SKU-42"),
            request
                .scope
                .source_listing_id
                .as_ref()
                .map(|id| id.as_ref())
        );
        assert_eq!(None, request.scope.lifecycle);
        assert_eq!(AdminProductListingSort::Updated, request.scope.sort);
        assert_eq!(SortOrder::Asc, request.scope.order);
        let cursor = AdminProductListingSearchCursor {
            product_listing_id: ProductListingId::new(),
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
            scope: request.scope,
        };
        assert_eq!(
            cursor,
            decode_cursor(encode_cursor(cursor.clone())).unwrap()
        );
        assert_eq!(21, parse("").unwrap().cursor.size);
        assert_eq!(100, parse("size=101").unwrap().cursor.size);
    }
    #[test]
    fn rejects_malformed_and_unbounded_directory_input() {
        for raw in [
            "query=%00",
            "lifecycle=UNKNOWN",
            "availability=SOLD",
            "sort=price",
            "order=random",
            "listingSourceId=no",
            "auctionId=no",
            "searchAfter=garbage",
            "extra=true",
            "sourceListingId=%20",
        ] {
            assert!(parse(raw).is_err(), "{raw}");
        }
        assert!(parse(&format!("query={}", "x".repeat(513))).is_err());
    }
}
