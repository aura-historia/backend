use super::types::{PaginatedData, SearchFilterData};
use super::util::no_store;
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_ORDER_VALUE, BAD_QUERY_PARAMETER_VALUE, BAD_SORT_VALUE};
use crate::state::SearchFiltersState;
use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use common::sort::SortOrder;
use search_filter_service::use_cases::ListOwnedSearchFiltersRequest;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListSearchFiltersQuery {
    sort: Option<String>,
    order: Option<String>,
}

pub(super) async fn list_search_filters(
    State(state): State<SearchFiltersState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let query: ListSearchFiltersQuery =
        match serde_qs::from_str(raw_query.as_deref().unwrap_or_default()) {
            Ok(value) => value,
            Err(error) => {
                return ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                    .with_detail(error.to_string())
                    .into_response();
            }
        };
    if let Some(sort) = query.sort {
        if sort != "created" {
            return ApiError::bad_request(BAD_SORT_VALUE)
                .with_query_field("sort")
                .with_detail("Expected 'created'.")
                .into_response();
        }
    }
    if let Some(order) = query.order {
        if SortOrder::try_from(order.as_str()).is_err() {
            return ApiError::bad_request(BAD_ORDER_VALUE)
                .with_query_field("order")
                .with_detail("Expected 'asc' or 'desc'.")
                .into_response();
        }
    }
    let (context, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match state
        .list_owned_search_filters
        .execute(&context, ListOwnedSearchFiltersRequest { user_id })
        .await
    {
        Ok(result) => {
            let size = result.items.len() as u64;
            no_store(
                Json(PaginatedData {
                    items: result
                        .items
                        .into_iter()
                        .map(SearchFilterData::from)
                        .collect(),
                    from: 0,
                    size,
                    total: Some(size),
                })
                .into_response(),
            )
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}
