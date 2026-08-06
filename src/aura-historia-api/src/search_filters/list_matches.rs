use super::util::{no_store, parse_search_filter_id};
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_QUERY_PARAMETER_VALUE, SEARCH_FILTER_INTERNAL_ERROR};
use crate::products::product_data::{
    PersonalizedProductDetailsData, personalized_product_details_data,
};
use crate::state::SearchFiltersState;
use axum::Json;
use axum::extract::{Path, RawQuery, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use common::language::data::LanguageData;
use common::pagination::cursor::{Cursor, CursoredResult, api::JsonCursoredData};
use common::product_id::ProductId;
use common::sort::SortOrder;
use search_filter_service::ports::SearchFilterMatchCursor;
use search_filter_service::use_cases::ListSearchFilterMatchesRequest;
use serde::Deserialize;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListSearchFilterMatchesQuery {
    #[serde(default)]
    sort: Option<SortSearchFilterMatchFieldData>,
    #[serde(default)]
    order: Option<SortOrder>,
    #[serde(default)]
    language: LanguageData,
    #[serde(default)]
    size: Option<SearchFilterMatchPageSize>,
    #[serde(default)]
    search_after: Option<SearchFilterMatchCursorData>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SortSearchFilterMatchFieldData {
    Created,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(transparent)]
struct SearchFilterMatchPageSize(u64);

#[derive(Debug, Clone, Deserialize)]
#[serde(transparent)]
struct SearchFilterMatchCursorData(serde_json::Value);

pub(super) async fn list_search_filter_matches(
    State(state): State<SearchFiltersState>,
    headers: HeaderMap,
    Path(raw_search_filter_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let search_filter_id = match parse_search_filter_id(&raw_search_filter_id) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let query: ListSearchFilterMatchesQuery =
        match serde_qs::from_str(raw_query.as_deref().unwrap_or_default()) {
            Ok(value) => value,
            Err(error) => {
                return ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                    .with_detail(error.to_string())
                    .into_response();
            }
        };
    let _sort = query
        .sort
        .unwrap_or(SortSearchFilterMatchFieldData::Created);
    let order = query.order.unwrap_or(SortOrder::Asc);
    let cursor = match matches_cursor(query.size, query.search_after) {
        Ok(cursor) => cursor,
        Err(error) => return error.into_response(),
    };
    let (context, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match state
        .list_search_filter_matches
        .execute(
            &context,
            ListSearchFilterMatchesRequest {
                user_id,
                search_filter_id,
                language: query.language.into(),
                cursor: Some(cursor),
                order,
            },
        )
        .await
    {
        Ok(result) => {
            let search_after = match result
                .cursor
                .search_after
                .map(matches_cursor_value)
                .transpose()
            {
                Ok(value) => value,
                Err(error) => return error.into_response(),
            };
            no_store(
                Json(JsonCursoredData::<PersonalizedProductDetailsData>::from(
                    CursoredResult {
                        items: result
                            .items
                            .into_iter()
                            .map(personalized_product_details_data)
                            .collect(),
                        cursor: Cursor {
                            size: result.cursor.size,
                            search_after,
                        },
                        total: result.total,
                    },
                ))
                .into_response(),
            )
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn matches_cursor(
    size: Option<SearchFilterMatchPageSize>,
    search_after: Option<SearchFilterMatchCursorData>,
) -> Result<Cursor<SearchFilterMatchCursor>, ApiError> {
    let size = size.map_or(21, |value| value.0).clamp(1, 100);
    let search_after = search_after
        .map(|value| parse_matches_cursor(value.0))
        .transpose()?;
    Ok(Cursor { size, search_after })
}

fn parse_matches_cursor(value: Value) -> Result<SearchFilterMatchCursor, ApiError> {
    let Value::Array(values) = value else {
        return Err(ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            .with_query_field("searchAfter")
            .with_detail("searchAfter must be a JSON array containing timestamp and product ID."));
    };
    let [Value::String(created), Value::String(product_id)] = values.as_slice() else {
        return Err(ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            .with_query_field("searchAfter")
            .with_detail("searchAfter must contain an RFC3339 timestamp and product UUID."));
    };
    let created = OffsetDateTime::parse(created, &Rfc3339).map_err(|error| {
        ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            .with_query_field("searchAfter")
            .with_detail(error.to_string())
    })?;
    let product_id = ProductId::try_from(product_id).map_err(|_| {
        ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            .with_query_field("searchAfter")
            .with_detail("searchAfter must contain a product UUID.")
    })?;
    Ok(SearchFilterMatchCursor {
        created,
        product_id,
    })
}

fn matches_cursor_value(cursor: SearchFilterMatchCursor) -> Result<Value, ApiError> {
    cursor
        .created
        .format(&Rfc3339)
        .map(|created| json!([created, cursor.product_id]))
        .map_err(|_| {
            ApiError::internal_server_error(SEARCH_FILTER_INTERNAL_ERROR)
                .with_detail("Search-filter match cursor failed internally.")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn should_parse_tie_safe_match_cursor() -> Result<(), Box<dyn std::error::Error>> {
        let product_id = ProductId::new();
        let cursor = parse_matches_cursor(json!(["2026-08-05T12:30:00Z", product_id]))?;

        assert_eq!(
            cursor.created,
            OffsetDateTime::parse("2026-08-05T12:30:00Z", &Rfc3339)?
        );
        assert_eq!(cursor.product_id, product_id);
        assert_eq!(
            matches_cursor_value(cursor)?,
            json!(["2026-08-05T12:30:00Z", product_id])
        );
        Ok(())
    }
}
