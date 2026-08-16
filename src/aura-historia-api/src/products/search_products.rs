use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{ApiError, BAD_ORDER_VALUE, BAD_QUERY_PARAMETER_VALUE, BAD_SORT_VALUE};
use crate::products::product_data::{
    PersonalizedProductSummaryData, personalized_product_summary_data,
};
use crate::state::ProductsState;
use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use common::currency::data::CurrencyData;
use common::distance::data::GeoDistanceQueryData;
use common::language::data::LanguageData;
use common::operation_context::Principal;
use common::pagination::cursor::Cursor;
use common::price::domain::MonetaryAmount;
use common::product_id::ProductId;
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_state::domain::ProductState;
use common::query::range_query::RangeQuery;
use common::query::text_query::TextQuery;
use common::seller_slug_id::SellerSlugId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::sort::{Sort, SortOrder};
use geo::data::continent_data::ContinentData;
use isocountry::CountryCode;
use product_core::product_search::{EnhancedSearchDescription, ProductSearch};
use product_core::sort_product_field::SortProductField;
use product_service::use_cases::SearchProductsRequest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use shop_core::shop_type::ShopType;
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProductSearchData {
    #[serde(default)]
    language: LanguageData,
    #[serde(default)]
    currency: CurrencyData,
    #[serde(default)]
    product_query: Vec<TextQuery<1>>,
    #[serde(default)]
    enhanced_search_description: Option<String>,
    #[serde(default)]
    exclude_product_id: HashSet<ProductId>,
    #[serde(default)]
    shop_name: HashSet<ShopName>,
    #[serde(default)]
    exclude_shop_name: HashSet<ShopName>,
    #[serde(default)]
    seller_name: HashSet<ShopName>,
    #[serde(default)]
    exclude_seller_name: HashSet<ShopName>,
    #[serde(default)]
    shop_slug_id: HashSet<ShopSlugId>,
    #[serde(default)]
    exclude_shop_slug_id: HashSet<ShopSlugId>,
    #[serde(default)]
    seller_slug_id: HashSet<SellerSlugId>,
    #[serde(default)]
    exclude_seller_slug_id: HashSet<SellerSlugId>,
    #[serde(default)]
    shop_type: HashSet<ShopTypeData>,
    #[serde(default)]
    country: HashSet<CountryCode>,
    #[serde(default)]
    continent: HashSet<ContinentData>,
    #[serde(default)]
    geo_address: Option<GeoDistanceQueryData>,
    #[serde(default)]
    price: Option<RangeQuery<u64>>,
    #[serde(default)]
    state: HashSet<ProductStateData>,
    #[serde(default)]
    lifecycle: HashSet<ProductLifecycleData>,
    #[serde(with = "common::query::range_query::range_rfc3339::option", default)]
    created: Option<RangeQuery<OffsetDateTime>>,
    #[serde(with = "common::query::range_query::range_rfc3339::option", default)]
    updated: Option<RangeQuery<OffsetDateTime>>,
    #[serde(with = "common::query::range_query::range_rfc3339::option", default)]
    auction_start: Option<RangeQuery<OffsetDateTime>>,
    #[serde(with = "common::query::range_query::range_rfc3339::option", default)]
    auction_end: Option<RangeQuery<OffsetDateTime>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ProductStateData {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ProductLifecycleData {
    Active,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ShopTypeData {
    AuctionHouse,
    AuctionPlatform,
    CommercialDealer,
    Marketplace,
}

#[derive(Debug, Clone, Copy)]
enum SortProductFieldData {
    Score,
    Updated,
    Created,
}

impl TryFrom<&str> for SortProductFieldData {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "score" => Ok(Self::Score),
            "updated" => Ok(Self::Updated),
            "created" => Ok(Self::Created),
            _ => Err("Expected any of: 'score', 'updated', 'created'.".to_owned()),
        }
    }
}

impl From<SortProductFieldData> for SortProductField {
    fn from(value: SortProductFieldData) -> Self {
        match value {
            SortProductFieldData::Score => Self::Score,
            SortProductFieldData::Updated => Self::Updated,
            SortProductFieldData::Created => Self::Created,
        }
    }
}

impl From<ProductSearchData> for ProductSearch {
    fn from(data: ProductSearchData) -> Self {
        Self {
            language: data.language.into(),
            currency: data.currency.into(),
            product_query: data.product_query,
            enhanced_search_description: data
                .enhanced_search_description
                .map(EnhancedSearchDescription::from),
            exclude_product_id_query: data.exclude_product_id.into(),
            shop_name_query: data.shop_name.into(),
            exclude_shop_name_query: data.exclude_shop_name.into(),
            seller_name_query: data.seller_name.into(),
            exclude_seller_name_query: data.exclude_seller_name.into(),
            shop_slug_id_query: data.shop_slug_id.into(),
            exclude_shop_slug_id_query: data.exclude_shop_slug_id.into(),
            seller_slug_id_query: data.seller_slug_id.into(),
            exclude_seller_slug_id_query: data.exclude_seller_slug_id.into(),
            shop_type_query: data.shop_type.into_iter().map(Into::into).collect(),
            country_query: data.country.into(),
            continent_query: data.continent.into_iter().map(Into::into).collect(),
            geo_address_distance_query: data.geo_address.map(Into::into),
            price_query: data.price.map(|range| range.map(MonetaryAmount::from)),
            state_query: data.state.into_iter().map(Into::into).collect(),
            lifecycle_query: data.lifecycle.into_iter().map(Into::into).collect(),
            created_query: data.created,
            updated_query: data.updated,
            auction_start_query: data.auction_start,
            auction_end_query: data.auction_end,
        }
    }
}

impl From<ProductStateData> for ProductState {
    fn from(data: ProductStateData) -> Self {
        match data {
            ProductStateData::Listed => Self::Listed,
            ProductStateData::Available => Self::Available,
            ProductStateData::Reserved => Self::Reserved,
            ProductStateData::Sold => Self::Sold,
            ProductStateData::Removed => Self::Removed,
            ProductStateData::Unknown => Self::Unknown,
        }
    }
}

impl From<ProductLifecycleData> for ProductLifecycle {
    fn from(data: ProductLifecycleData) -> Self {
        match data {
            ProductLifecycleData::Active => Self::Active,
            ProductLifecycleData::Deleted => Self::Deleted,
        }
    }
}

impl From<ShopTypeData> for ShopType {
    fn from(data: ShopTypeData) -> Self {
        match data {
            ShopTypeData::AuctionHouse => Self::AuctionHouse,
            ShopTypeData::AuctionPlatform => Self::AuctionPlatform,
            ShopTypeData::CommercialDealer => Self::CommercialDealer,
            ShopTypeData::Marketplace => Self::Marketplace,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CursoredProductsData {
    items: Vec<PersonalizedProductSummaryData>,
    size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    search_after: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total: Option<u64>,
}

pub async fn get_products(
    State(state): State<ProductsState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let data = match serde_qs::from_str(raw_query.as_deref().unwrap_or_default()) {
        Ok(data) => data,
        Err(error) => {
            return ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                .with_detail(error.to_string())
                .into_response();
        }
    };
    handle_search(state, headers, raw_query.as_deref(), data).await
}

async fn handle_search(
    state: ProductsState,
    headers: HeaderMap,
    raw_query: Option<&str>,
    data: ProductSearchData,
) -> Response {
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let sort = match parse_sort(raw_query) {
        Ok(sort) => sort,
        Err(error) => return error.into_response(),
    };
    let cursor = match parse_cursor(raw_query) {
        Ok(cursor) => cursor,
        Err(error) => return error.into_response(),
    };

    let context = principal.operation_context(metadata);
    match state
        .search_products
        .execute(
            &context,
            SearchProductsRequest {
                search: data.into(),
                sort,
                cursor,
            },
        )
        .await
    {
        Ok(result) => {
            let mut response = Json(CursoredProductsData {
                items: result
                    .items
                    .into_iter()
                    .map(personalized_product_summary_data)
                    .collect(),
                size: result.cursor.size,
                search_after: result.cursor.search_after,
                total: result.total,
            })
            .into_response();
            let value = match context.principal {
                Principal::Anonymous => "public, max-age=60, s-maxage=300",
                Principal::User(_)
                | Principal::DelegatedUser { .. }
                | Principal::Service(_)
                | Principal::System => "no-store",
            };
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static(value));
            response
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn parse_sort(raw_query: Option<&str>) -> Result<Option<Sort<SortProductField>>, ApiError> {
    match (
        query_value(raw_query, "sort"),
        query_value(raw_query, "order"),
    ) {
        (Some(sort), Some(order)) => {
            let sort = SortProductFieldData::try_from(sort.as_str()).map_err(|detail| {
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

fn parse_cursor(raw_query: Option<&str>) -> Result<Option<Cursor<Value>>, ApiError> {
    let size = query_value(raw_query, "size")
        .map(|value| value.parse::<u64>())
        .transpose()
        .map_err(|error| {
            ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                .with_query_field("size")
                .with_detail(error.to_string())
        })?
        .map(|size| size.clamp(1, 100));
    let values = query_values(raw_query, "searchAfter");
    let search_after = match values.len() {
        0 => None,
        1 => Some(parse_search_after(&values[0])?),
        _ => Some(Value::Array(
            values
                .iter()
                .map(|value| parse_search_after(value))
                .collect::<Result<Vec<_>, _>>()?,
        )),
    };

    if size.is_some() || search_after.is_some() {
        Ok(Some(Cursor {
            size: size.unwrap_or_else(|| Cursor::<Value>::default().size),
            search_after,
        }))
    } else {
        Ok(None)
    }
}

fn parse_search_after(value: &str) -> Result<Value, ApiError> {
    let json = match value {
        "null" | "true" | "false" => value.to_owned(),
        _ if value.starts_with('[') || value.starts_with('{') || value.parse::<f64>().is_ok() => {
            value.to_owned()
        }
        _ => serde_json::to_string(value).map_err(|error| {
            ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE).with_detail(error.to_string())
        })?,
    };
    serde_json::from_str(&json).map_err(|error| {
        ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            .with_query_field("searchAfter")
            .with_detail(error.to_string())
    })
}

fn query_value(raw_query: Option<&str>, key: &str) -> Option<String> {
    query_values(raw_query, key).into_iter().next()
}

fn query_values(raw_query: Option<&str>, key: &str) -> Vec<String> {
    raw_query
        .map(|query| {
            url::form_urlencoded::parse(query.as_bytes())
                .filter(|(name, _)| name == key)
                .map(|(_, value)| value.into_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthError, RequestMetadata, TokenAuthenticator, TransportPrincipal};
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use common::currency::domain::Currency;
    use common::language::domain::Language;
    use common::operation_context::OperationContext;
    use common::pagination::cursor::Cursor;
    use common::sort::SortOrder;
    use product_service::use_cases::{
        GetProductError, GetProductRequest, GetProductUseCase, GetSimilarProductsError,
        GetSimilarProductsRequest, GetSimilarProductsResult, GetSimilarProductsUseCase,
        SearchProductsError, SearchProductsResult, SearchProductsUseCase,
    };
    use serde_json::Value;
    use std::sync::{Arc, Mutex, MutexGuard};
    use tower::ServiceExt;

    type SearchProductsCalls = Arc<Mutex<Vec<(OperationContext, SearchProductsRequest)>>>;

    struct UnusedGetProductUseCase;

    #[async_trait::async_trait]
    impl GetProductUseCase for UnusedGetProductUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: GetProductRequest,
        ) -> Result<product_service::use_cases::PersonalizedProductDetailsView, GetProductError>
        {
            Err(GetProductError::NotFound)
        }
    }

    struct UnusedSimilarProductsUseCase;

    #[async_trait::async_trait]
    impl GetSimilarProductsUseCase for UnusedSimilarProductsUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: GetSimilarProductsRequest,
        ) -> Result<GetSimilarProductsResult, GetSimilarProductsError> {
            Err(GetSimilarProductsError::SimilaritySearchUnavailable)
        }
    }

    #[derive(Clone)]
    struct FakeSearchProductsUseCase {
        calls: SearchProductsCalls,
    }

    #[async_trait::async_trait]
    impl SearchProductsUseCase for FakeSearchProductsUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            request: SearchProductsRequest,
        ) -> Result<SearchProductsResult, SearchProductsError> {
            lock(&self.calls).push((context.clone(), request));
            Ok(SearchProductsResult::default())
        }
    }

    struct AnonymousAuthenticator;

    #[async_trait::async_trait]
    impl TokenAuthenticator for AnonymousAuthenticator {
        async fn authenticate(
            &self,
            _bearer_token: &str,
            _metadata: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            Ok(TransportPrincipal::Anonymous)
        }
    }

    #[tokio::test]
    async fn should_map_get_search_request_and_add_public_cache_header()
    -> Result<(), Box<dyn std::error::Error>> {
        let (app, calls) = app();

        let response = app
            .oneshot(
                Request::get(
                    "/api/v1/products?language=de&currency=USD&productQuery=cabinet&sort=updated&order=desc&size=200&searchAfter=next",
                )
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            "public, max-age=60, s-maxage=300",
            response.headers()[header::CACHE_CONTROL]
        );
        let calls = lock(&calls);
        assert_eq!(1, calls.len());
        let request = &calls[0].1;
        assert_eq!(Language::De, request.search.language);
        assert_eq!(Currency::Usd, request.search.currency);
        assert_eq!("cabinet", request.search.product_query[0].as_ref());
        assert!(matches!(
            request.sort,
            Some(Sort {
                sort: SortProductField::Updated,
                order: SortOrder::Desc,
            })
        ));
        assert_eq!(
            Some(Cursor {
                size: 100,
                search_after: Some(Value::String("next".to_owned())),
            }),
            request.cursor
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_price_sort() -> Result<(), Box<dyn std::error::Error>> {
        let (app, calls) = app();

        let response = app
            .oneshot(Request::get("/api/v1/products?sort=price&order=asc").body(Body::empty())?)
            .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert!(lock(&calls).is_empty());
        Ok(())
    }

    fn app() -> (Router, SearchProductsCalls) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let state = ProductsState::new(
            Arc::new(UnusedGetProductUseCase),
            Arc::new(UnusedSimilarProductsUseCase),
            Arc::new(FakeSearchProductsUseCase {
                calls: Arc::clone(&calls),
            }),
            Arc::new(AnonymousAuthenticator),
        );
        (
            Router::new()
                .route("/api/v1/products", axum::routing::get(get_products))
                .with_state(state),
            calls,
        )
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}
