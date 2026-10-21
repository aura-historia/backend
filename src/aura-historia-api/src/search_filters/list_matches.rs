use super::types::{SearchFilterMatchesData, SearchFilterMatchesQuery};
use super::util::{no_store, parse_json_query, parse_search_filter_id};
use crate::{
    auth::protected_context,
    error::{ApiError, BAD_QUERY_PARAMETER_VALUE, SEARCH_FILTER_INTERNAL_ERROR},
    state::SearchFiltersState,
};
use axum::{
    Json,
    extract::{Path, RawQuery, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListSearchFilterMatchesQuery {
    #[serde(flatten)]
    query: SearchFilterMatchesQuery,
    #[serde(default)]
    search_after: Option<String>,
    // serde_qs scalar coercion is lost inside Serde's flattened map. Keep the
    // original numeric extractor at the HTTP boundary, then reuse shared paging.
    #[serde(default)]
    size: Option<SearchFilterMatchPageSize>,
}
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(transparent)]
struct SearchFilterMatchPageSize(u64);

pub(super) async fn list_search_filter_matches(
    State(state): State<SearchFiltersState>,
    headers: HeaderMap,
    Path(raw_search_filter_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let search_filter_id = match parse_search_filter_id(&raw_search_filter_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let mut query: ListSearchFilterMatchesQuery =
        match crate::wire::from_query_str(raw_query.as_deref().unwrap_or_default()) {
            Ok(query) => query,
            Err(error) => {
                return ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                    .with_detail(error.to_string())
                    .into_response();
            }
        };
    query.query.size = query.size.map(|size| size.0);
    let search_after = match query
        .search_after
        .as_deref()
        .map(|raw| parse_json_query::<Value>(raw, "searchAfter"))
        .transpose()
    {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    let cursor = match query.query.cursor(search_after) {
        Ok(cursor) => cursor,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let (context, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let request = search_filter_service::use_cases::ListSearchFilterMatchesRequest {
        user_id,
        search_filter_id,
        language: query.query.language,
        currency: query.query.currency,
        cursor: Some(cursor),
        order: domain_primitives::sort::SortOrder::Asc,
    };
    match state
        .list_search_filter_matches
        .execute(&context, request)
        .await
    {
        Ok(result) => match SearchFilterMatchesData::try_from(result) {
            Ok(data) => no_store(Json(data).into_response()),
            Err(_) => ApiError::internal_server_error(SEARCH_FILTER_INTERNAL_ERROR)
                .with_detail("Search-filter match cursor failed internally.")
                .into_response(),
        },
        Err(error) => ApiError::from(error).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_historia_contracts::saved_searches::matches_cursor_value;
    use listing_source_core::ListingSourceId;
    use localization::Language;
    use money::Currency;
    use product_listing_core::product_listing_id::ProductListingId;
    use serde_json::json;
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};
    fn matches_cursor(
        size: Option<u64>,
        search_after: Option<&str>,
    ) -> Result<
        application::pagination::Cursor<search_filter_service::ports::SearchFilterMatchCursor>,
        ApiError,
    > {
        let value = search_after
            .map(|raw| parse_json_query(raw, "searchAfter"))
            .transpose()?;
        let query = SearchFilterMatchesQuery {
            language: Language::En,
            currency: Currency::Eur,
            size,
        };
        Ok(query
            .into_request(Default::default(), Default::default(), value)?
            .cursor
            .unwrap())
    }
    fn parse_matches_cursor(
        raw: &str,
    ) -> Result<search_filter_service::ports::SearchFilterMatchCursor, ApiError> {
        aura_historia_contracts::saved_searches::parse_matches_cursor(parse_json_query(
            raw,
            "searchAfter",
        )?)
        .map_err(Into::into)
    }

    #[test]
    fn should_default_match_currency_to_eur_and_parse_requested_currency()
    -> Result<(), Box<dyn std::error::Error>> {
        let default_query: ListSearchFilterMatchesQuery = crate::wire::from_query_str("")?;
        let requested_query: ListSearchFilterMatchesQuery =
            crate::wire::from_query_str("currency=USD")?;

        assert_eq!(Currency::Eur, default_query.query.currency);
        assert_eq!(Currency::Usd, requested_query.query.currency);
        Ok(())
    }

    #[test]
    fn should_clamp_match_page_size_after_defaulting() -> Result<(), Box<dyn std::error::Error>> {
        for (size, expected) in [
            (None, 21),
            (Some(0), 1),
            (Some(1), 1),
            (Some(21), 21),
            (Some(100), 100),
            (Some(101), 100),
        ] {
            assert_eq!(expected, matches_cursor(size, None)?.size);
        }
        Ok(())
    }

    #[test]
    fn should_parse_numeric_page_size_before_flattening_shared_presentation() {
        for (raw, expected) in [("size=0", 0), ("size=1", 1), ("size=101", 101)] {
            let parsed: ListSearchFilterMatchesQuery = crate::wire::from_query_str(raw).unwrap();
            assert_eq!(expected, parsed.size.unwrap().0);
        }
    }

    #[test]
    fn should_reject_malformed_or_negative_match_page_size_query() {
        for raw_query in ["size=not-an-integer", "size=-1"] {
            assert!(
                crate::wire::from_query_str::<ListSearchFilterMatchesQuery>(raw_query).is_err()
            );
        }
    }

    #[test]
    fn should_parse_tie_safe_match_cursor() -> Result<(), Box<dyn std::error::Error>> {
        let product_listing_id = ProductListingId::new();
        let raw_cursor =
            serde_json::to_string(&json!(["2026-08-05T12:30:00Z", product_listing_id]))?;
        let cursor = parse_matches_cursor(&raw_cursor)?;

        assert_eq!(
            cursor.created,
            OffsetDateTime::parse("2026-08-05T12:30:00Z", &Rfc3339)?
        );
        assert_eq!(cursor.product_listing_id, product_listing_id);
        assert_eq!(
            matches_cursor_value(cursor)?,
            json!(["2026-08-05T12:30:00Z", product_listing_id])
        );
        for raw in [
            "not-json".to_owned(),
            "{}".to_owned(),
            "[]".to_owned(),
            json!(["only-one-value"]).to_string(),
            json!(["bad-date", product_listing_id]).to_string(),
        ] {
            let Err(error) = parse_matches_cursor(&raw) else {
                return Err("malformed match cursor was accepted".into());
            };
            assert_eq!(BAD_QUERY_PARAMETER_VALUE, error.code());
        }
        for invalid_id in [
            ListingSourceId::new().to_string(),
            product_listing_id.as_uuid().to_string(),
            "pl_not-a-typeid".to_owned(),
        ] {
            let raw = json!(["2026-08-05T12:30:00Z", invalid_id]).to_string();
            let Err(error) = parse_matches_cursor(&raw) else {
                return Err("noncanonical ProductListing cursor ID was accepted".into());
            };
            assert_eq!(crate::error::INVALID_OBJECT_ID, error.code());
        }
        Ok(())
    }

    #[test]
    fn should_document_only_persisted_match_route() {
        let swagger = include_str!("../../../../docs/swagger.yaml");

        assert!(swagger.contains("/v1/me/search-filters/{userSearchFilterId}/matches:"));
        assert!(swagger.contains("operationId: listSearchFilterMatches"));
        assert!(!swagger.contains("/v1/me/search-filters/{userSearchFilterId}/product-listings:"));
        assert!(!swagger.contains("getSearchFilterPreviewProductListings"));
    }
}
