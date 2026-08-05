use super::types::{CursoredData, SearchFilterMatchData};
use super::util::{no_store, parse_rfc3339_query, parse_search_filter_id};
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_ORDER_VALUE, BAD_QUERY_PARAMETER_VALUE, BAD_SORT_VALUE};
use crate::state::SearchFiltersState;
use axum::Json;
use axum::extract::{Path, RawQuery, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use common::pagination::cursor::Cursor;
use common::sort::SortOrder;
use search_filter_service::use_cases::ListSearchFilterMatchesRequest;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListSearchFilterMatchesQuery {
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    order: Option<String>,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    // Legacy query fields. Canonical matches now return match read models, not products.
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    currency: Option<String>,
}

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
    let _ = (query.language.as_deref(), query.currency.as_deref());
    if let Some(sort) = query.sort {
        if sort != "created" {
            return ApiError::bad_request(BAD_SORT_VALUE)
                .with_query_field("sort")
                .with_detail("Expected 'created'.")
                .into_response();
        }
    }
    let order = match query.order {
        Some(value) => match SortOrder::try_from(value.as_str()) {
            Ok(value) => value,
            Err(error) => {
                return ApiError::bad_request(BAD_ORDER_VALUE)
                    .with_query_field("order")
                    .with_detail(error)
                    .into_response();
            }
        },
        None => SortOrder::Asc,
    };
    let search_after = match query.from {
        Some(value) => match parse_rfc3339_query(&value, "from") {
            Ok(value) => Some(value),
            Err(error) => return error.into_response(),
        },
        None => None,
    };
    let cursor = query.size.or(search_after.map(|_| 0)).map(|size| Cursor {
        size: size.clamp(1, 100),
        search_after,
    });
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
                cursor,
                order,
            },
        )
        .await
    {
        Ok(result) => no_store(
            Json(CursoredData {
                items: result
                    .matches
                    .items
                    .into_iter()
                    .map(SearchFilterMatchData::from)
                    .collect(),
                size: result.matches.cursor.size,
                search_after: result.matches.cursor.search_after,
                total: result.matches.total,
            })
            .into_response(),
        ),
        Err(error) => ApiError::from(error).into_response(),
    }
}
