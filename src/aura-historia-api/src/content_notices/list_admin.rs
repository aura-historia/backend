use super::{no_store, types::ContentNoticeSummaryData};
use crate::{
    auth::protected_context,
    error::{ApiError, BAD_ORDER_VALUE, BAD_QUERY_PARAMETER_VALUE, BAD_SORT_VALUE},
    keyset_cursor,
    pagination_data::JsonCursoredData,
    state::ContentNoticesState,
    wire::parse_query_object_id,
};
use application::pagination::Cursor;
use axum::{
    Json,
    extract::{RawQuery, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use content_notice_core::{
    content_notice_target::ContentNoticeTargetType, reason_category::ReasonCategory,
    submitter_role::SubmitterRole,
};
use content_notice_service::use_cases::queries::list_admin_content_notices::{
    ContentNoticeCursor, ContentNoticeSearch, ContentNoticeSortKey, ListAdminContentNoticesRequest,
    SortContentNoticeField,
};
use domain_primitives::{
    query::range_query::RangeQuery,
    sort::{Sort, SortOrder},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListAdminContentNoticesQuery {
    #[serde(default)]
    target_type: Option<String>,
    #[serde(default)]
    listing_source_id: Option<String>,
    #[serde(default)]
    product_listing_id: Option<String>,
    #[serde(default)]
    reason_category: Option<String>,
    #[serde(default)]
    submitter_role: Option<String>,
    #[serde(
        default,
        with = "domain_primitives::query::range_query::range_rfc3339::option"
    )]
    created: Option<RangeQuery<OffsetDateTime>>,
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    order: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    search_after: Option<String>,
}

pub(super) async fn list_admin(
    State(state): State<ContentNoticesState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let request = match parse_list_admin_query(raw_query.as_deref()) {
        Ok(request) => request,
        Err(error) => return no_store(error.into_response()),
    };

    match state.list_admin.execute(&context, request).await {
        Ok(result) => {
            let search_after = result
                .cursor
                .search_after
                .map(|cursor| Value::String(encode_search_after(cursor)));
            let items = result
                .items
                .into_iter()
                .map(ContentNoticeSummaryData::from)
                .collect();
            no_store(Json(JsonCursoredData::new(items, search_after, None)).into_response())
        }
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

fn parse_list_admin_query(
    raw_query: Option<&str>,
) -> Result<ListAdminContentNoticesRequest, ApiError> {
    let query: ListAdminContentNoticesQuery =
        crate::wire::from_query_str(raw_query.unwrap_or_default()).map_err(|error| {
            ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE).with_detail(error.to_string())
        })?;

    let search = ContentNoticeSearch {
        target_type: parse_code(
            query.target_type,
            "targetType",
            ContentNoticeTargetType::from_code,
        )?,
        listing_source_id: query
            .listing_source_id
            .map(|value| parse_query_object_id(&value, "listingSourceId", "ListingSource"))
            .transpose()?,
        product_listing_id: query
            .product_listing_id
            .map(|value| parse_query_object_id(&value, "productListingId", "ProductListing"))
            .transpose()?,
        reason_category: parse_code(
            query.reason_category,
            "reasonCategory",
            ReasonCategory::from_code,
        )?,
        submitter_role: parse_code(
            query.submitter_role,
            "submitterRole",
            SubmitterRole::from_code,
        )?,
        created: query.created,
    };
    let sort = parse_sort(query.sort.as_deref(), query.order.as_deref())?;
    let cursor = parse_cursor(query.size.as_deref(), query.search_after.as_deref())?;

    Ok(ListAdminContentNoticesRequest {
        search,
        sort,
        cursor,
    })
}

fn parse_code<T>(
    value: Option<String>,
    field: &'static str,
    from_code: fn(&str) -> Option<T>,
) -> Result<Option<T>, ApiError> {
    value
        .map(|value| {
            from_code(&value)
                .ok_or_else(|| bad_query(field, format!("Unsupported {field} '{value}'.")))
        })
        .transpose()
}

/// `sort` and `order` come together or not at all; without them the list is newest first.
fn parse_sort(
    sort: Option<&str>,
    order: Option<&str>,
) -> Result<Option<Sort<SortContentNoticeField>>, ApiError> {
    let sort = sort
        .map(|value| match value {
            "created" => Ok(SortContentNoticeField::Created),
            value => Err(ApiError::bad_request(BAD_SORT_VALUE)
                .with_query_field("sort")
                .with_detail(format!("Expected any of: 'created'. Got: '{value}'"))),
        })
        .transpose()?;
    let order = order
        .map(|value| {
            SortOrder::try_from(value).map_err(|detail| {
                ApiError::bad_request(BAD_ORDER_VALUE)
                    .with_query_field("order")
                    .with_detail(detail)
            })
        })
        .transpose()?;

    match (sort, order) {
        (None, None) => Ok(None),
        (Some(sort), Some(order)) => Ok(Some(Sort { sort, order })),
        (Some(_), None) => Err(ApiError::bad_request(BAD_ORDER_VALUE)
            .with_query_field("order")
            .with_detail("'order' is required when 'sort' is supplied.")),
        (None, Some(_)) => Err(ApiError::bad_request(BAD_SORT_VALUE)
            .with_query_field("sort")
            .with_detail("'sort' is required when 'order' is supplied.")),
    }
}

fn parse_cursor(
    size: Option<&str>,
    search_after: Option<&str>,
) -> Result<Option<Cursor<ContentNoticeCursor>>, ApiError> {
    let size = size
        .map(|value| value.parse::<u64>().map(|size| size.clamp(1, 100)))
        .transpose()
        .map_err(|error| bad_query("size", error))?;
    let search_after = search_after.map(parse_search_after).transpose()?;

    if size.is_none() && search_after.is_none() {
        return Ok(None);
    }
    Ok(Some(Cursor {
        size: size.unwrap_or_else(|| Cursor::<ContentNoticeCursor>::default().size),
        search_after,
    }))
}

fn parse_search_after(value: &str) -> Result<ContentNoticeCursor, ApiError> {
    let cursor = keyset_cursor::decode::<ContentNoticeSortKeyData, _>(value)?;
    // A forged position outside what PostgreSQL stores is client input, not an outage.
    let ContentNoticeSortKeyData::Created(created) = cursor.sort_key;
    if !(1..=9999).contains(&created.year()) {
        return Err(bad_query(
            "searchAfter",
            "searchAfter must be a cursor returned by this search.",
        ));
    }
    Ok(cursor.map_sort_key(Into::into))
}

fn encode_search_after(cursor: ContentNoticeCursor) -> String {
    keyset_cursor::encode(cursor.map_sort_key(ContentNoticeSortKeyData::from))
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum ContentNoticeSortKeyData {
    Created(#[serde(with = "time::serde::timestamp::nanoseconds")] OffsetDateTime),
}

impl From<ContentNoticeSortKey> for ContentNoticeSortKeyData {
    fn from(key: ContentNoticeSortKey) -> Self {
        match key {
            ContentNoticeSortKey::Created(created) => Self::Created(created),
        }
    }
}

impl From<ContentNoticeSortKeyData> for ContentNoticeSortKey {
    fn from(key: ContentNoticeSortKeyData) -> Self {
        match key {
            ContentNoticeSortKeyData::Created(created) => Self::Created(created),
        }
    }
}

fn bad_query(field: &'static str, detail: impl std::fmt::Display) -> ApiError {
    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
        .with_query_field(field)
        .with_detail(detail.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::pagination::ScopeBinding;
    use content_notice_core::content_notice_id::ContentNoticeId;
    use listing_source_core::ListingSourceId;
    use product_listing_core::product_listing_id::ProductListingId;
    use serde_json::json;
    use time::macros::datetime;

    fn error_code(query: &str) -> Value {
        let error = parse_list_admin_query(Some(query))
            .err()
            .unwrap_or_else(|| panic!("query should be rejected: {query}"));
        serde_json::to_value(error)
            .unwrap_or_else(|error| panic!("failed to serialize API error: {error}"))["error"]
            .clone()
    }

    #[test]
    fn should_map_every_query_parameter_to_the_service_request() -> Result<(), ApiError> {
        let listing_source_id = ListingSourceId::new();
        let product_listing_id = ProductListingId::new();
        let cursor = ContentNoticeCursor {
            scope: ScopeBinding::builder("test/v1").finish(),
            sort_key: ContentNoticeSortKey::Created(datetime!(2026-11-13 10:44:18.123456 UTC)),
            id: ContentNoticeId::new(),
        };

        let request = parse_list_admin_query(Some(&format!(
            "targetType=PRODUCT_LISTING&listingSourceId={listing_source_id}&productListingId={product_listing_id}&reasonCategory=RIGHTS_INFRINGEMENT&submitterRole=RIGHTS_HOLDER&created%5Bmin%5D=2026-01-01T00%3A00%3A00Z&created%5Bmax%5D=2026-12-31T23%3A59%3A59Z&sort=created&order=asc&size=200&searchAfter={}",
            encode_search_after(cursor.clone()),
        )))?;

        assert_eq!(
            ContentNoticeSearch {
                target_type: Some(ContentNoticeTargetType::ProductListing),
                listing_source_id: Some(listing_source_id),
                product_listing_id: Some(product_listing_id),
                reason_category: Some(ReasonCategory::RightsInfringement),
                submitter_role: Some(SubmitterRole::RightsHolder),
                created: Some(RangeQuery {
                    min: Some(datetime!(2026-01-01 00:00 UTC)),
                    max: Some(datetime!(2026-12-31 23:59:59 UTC)),
                }),
            },
            request.search
        );
        assert_eq!(
            Some(Sort {
                sort: SortContentNoticeField::Created,
                order: SortOrder::Asc,
            }),
            request.sort
        );
        assert_eq!(
            Some(Cursor {
                size: 100,
                search_after: Some(cursor),
            }),
            request.cursor
        );
        Ok(())
    }

    #[test]
    fn should_leave_sort_and_page_to_the_defaults_when_absent() -> Result<(), ApiError> {
        assert_eq!(
            ListAdminContentNoticesRequest::default(),
            parse_list_admin_query(None)?
        );
        assert_eq!(
            Some(1),
            parse_list_admin_query(Some("size=0"))?
                .cursor
                .map(|cursor| cursor.size)
        );
        Ok(())
    }

    #[test]
    fn should_require_sort_and_order_together() {
        assert_eq!(json!("BAD_ORDER_VALUE"), error_code("sort=created"));
        assert_eq!(json!("BAD_SORT_VALUE"), error_code("order=desc"));
    }

    #[test]
    fn should_reject_invalid_values() {
        for (query, expected) in [
            ("targetType=listing_source", "BAD_QUERY_PARAMETER_VALUE"),
            ("reasonCategory=SPAM", "BAD_QUERY_PARAMETER_VALUE"),
            ("submitterRole=ADMIN", "BAD_QUERY_PARAMETER_VALUE"),
            (
                "listingSourceId=pl_01h455vb4pex5vy7enb1p677vn",
                "INVALID_OBJECT_ID",
            ),
            ("productListingId=not-an-object-id", "INVALID_OBJECT_ID"),
            ("created%5Bmin%5D=yesterday", "BAD_QUERY_PARAMETER_VALUE"),
            ("sort=updated&order=asc", "BAD_SORT_VALUE"),
            ("sort=created&order=sideways", "BAD_ORDER_VALUE"),
            ("size=-1", "BAD_QUERY_PARAMETER_VALUE"),
            (
                "searchAfter=ntc_01h455vb4pex5vy7enb1p677vn",
                "BAD_QUERY_PARAMETER_VALUE",
            ),
        ] {
            assert_eq!(json!(expected), error_code(query), "query: {query}");
        }
    }
}
