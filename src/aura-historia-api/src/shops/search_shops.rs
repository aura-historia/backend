use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{
    ApiError, BAD_ORDER_VALUE, BAD_QUERY_PARAMETER_VALUE, BAD_SORT_VALUE, INVALID_UUID,
};
use crate::shops::shop_data::{ShopSummaryData, cache_control};
use crate::shops::types::{ShopContinentData, ShopPartnerStatusData, ShopTypeData};
use crate::state::ShopsState;
use application::pagination::Cursor;
use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use domain_primitives::query::range_query::RangeQuery;
use domain_primitives::query::text_query::TextQuery;
use domain_primitives::sort::{Sort, SortOrder};
use isocountry::CountryCode;
use serde::{Deserialize, Serialize};
use shop_core::continent::Continent;
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop_id::ShopId;
use shop_core::shop_type::ShopType;
use shop_core::sort_shop_field::SortShopField;
use shop_service::shop_search::ShopSearch;
use shop_service::use_cases::queries::search_shops::SearchShopsRequest;
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShopSearchData {
    #[serde(default)]
    shop_name_query: Option<TextQuery<0>>,
    #[serde(rename = "shopType", default)]
    shop_type_query: HashSet<ShopTypeData>,
    #[serde(rename = "partnerStatus", default)]
    partner_status_query: HashSet<ShopPartnerStatusData>,
    #[serde(default)]
    countries: HashSet<CountryCode>,
    #[serde(default)]
    continents: HashSet<ShopContinentData>,
    #[serde(
        with = "domain_primitives::query::range_query::range_rfc3339::option",
        default
    )]
    created: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        with = "domain_primitives::query::range_query::range_rfc3339::option",
        default
    )]
    updated: Option<RangeQuery<OffsetDateTime>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SortShopFieldData {
    Score,
    Name,
    Updated,
    Created,
}

impl TryFrom<&str> for SortShopFieldData {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "score" => Ok(Self::Score),
            "name" => Ok(Self::Name),
            "updated" => Ok(Self::Updated),
            "created" => Ok(Self::Created),
            invalid => Err(format!(
                "Expected any of: 'score', 'name', 'updated', 'created'. Got: '{invalid}'"
            )),
        }
    }
}

impl From<SortShopFieldData> for SortShopField {
    fn from(value: SortShopFieldData) -> Self {
        match value {
            SortShopFieldData::Score | SortShopFieldData::Name => SortShopField::Name,
            SortShopFieldData::Updated => SortShopField::Updated,
            SortShopFieldData::Created => SortShopField::Created,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonCursoredData<T> {
    items: Vec<T>,
    size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    search_after: Option<ShopId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total: Option<u64>,
}

pub async fn get_shops(
    State(state): State<ShopsState>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    handle_search(state, headers, query.as_deref()).await
}

async fn handle_search(state: ShopsState, headers: HeaderMap, raw_query: Option<&str>) -> Response {
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let context = principal.operation_context(metadata);
    let search = match parse_search(raw_query) {
        Ok(search) => search,
        Err(error) => return error.into_response(),
    };
    let sort = match parse_sort(raw_query) {
        Ok(sort) => sort,
        Err(error) => return error.into_response(),
    };
    let cursor = match parse_cursor(raw_query) {
        Ok(cursor) => cursor,
        Err(error) => return error.into_response(),
    };
    let result = match state
        .search_shops
        .execute(
            &context,
            SearchShopsRequest {
                search,
                sort,
                cursor,
            },
        )
        .await
    {
        Ok(result) => result,
        Err(error) => return ApiError::from(error).into_response(),
    };

    let mut response = Json(JsonCursoredData {
        items: result
            .items
            .into_iter()
            .map(ShopSummaryData::from)
            .collect(),
        size: result.cursor.size,
        search_after: result.cursor.search_after,
        total: result.total,
    })
    .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control(&context.principal)),
    );
    response
}

fn parse_search(raw_query: Option<&str>) -> Result<ShopSearch, ApiError> {
    let data: ShopSearchData =
        serde_qs::from_str(raw_query.unwrap_or_default()).map_err(|error| {
            ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE).with_detail(error.to_string())
        })?;
    Ok(ShopSearch {
        shop_name_query: data.shop_name_query,
        shop_type_query: data
            .shop_type_query
            .into_iter()
            .map(ShopType::from)
            .collect(),
        partner_status_query: data
            .partner_status_query
            .into_iter()
            .map(ShopPartnerStatus::from)
            .collect(),
        countries: data.countries.into_iter().collect(),
        continents: data.continents.into_iter().map(Continent::from).collect(),
        created: data.created,
        updated: data.updated,
    })
}

fn parse_sort(raw_query: Option<&str>) -> Result<Option<Sort<SortShopField>>, ApiError> {
    let sort = query_value(raw_query, "sort");
    let order = query_value(raw_query, "order");
    match (sort, order) {
        (Some(sort), Some(order)) => {
            let sort = SortShopFieldData::try_from(sort.as_str()).map_err(|detail| {
                ApiError::bad_request(BAD_SORT_VALUE)
                    .with_query_field("sort")
                    .with_detail(detail)
            })?;
            let order = SortOrder::try_from(order.as_str()).map_err(|detail| {
                ApiError::bad_request(BAD_ORDER_VALUE)
                    .with_query_field("order")
                    .with_detail(detail)
            })?;
            Ok(Some(Sort {
                sort: sort.into(),
                order,
            }))
        }
        _ => Ok(None),
    }
}

fn parse_cursor(raw_query: Option<&str>) -> Result<Option<Cursor<ShopId>>, ApiError> {
    let size = query_value(raw_query, "size")
        .map(|value| value.parse::<u64>())
        .transpose()
        .map_err(|error| {
            ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                .with_query_field("size")
                .with_detail(error.to_string())
        })?
        .map(|size| size.clamp(1, 100));
    let search_after = query_value(raw_query, "searchAfter")
        .map(|value| ShopId::try_from(value.as_str()))
        .transpose()
        .map_err(|_| {
            ApiError::bad_request(INVALID_UUID)
                .with_query_field("searchAfter")
                .with_detail("Query parameter 'searchAfter' must be a UUID.")
        })?;
    if size.is_some() || search_after.is_some() {
        Ok(Some(Cursor {
            size: size.unwrap_or_else(|| Cursor::<ShopId>::default().size),
            search_after,
        }))
    } else {
        Ok(None)
    }
}

fn query_value(raw_query: Option<&str>, key: &str) -> Option<String> {
    raw_query.and_then(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.into_owned())
    })
}
