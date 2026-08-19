use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{ApiError, BAD_PATH_PARAMETER_VALUE, INVALID_UUID, PRODUCT_INTERNAL_ERROR};
use crate::products::product_data::personalized_product_summary_data;
use crate::state::ProductsState;
use axum::Json;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use common::product_id::ProductId;
use common::product_slug_id::ProductSlugId;
use common::shop_slug_id::ShopSlugId;
use common::{currency::data::CurrencyData, language::data::LanguageData};
use product_service::ports::ProductEmbeddingLookup;
use product_service::use_cases::{GetSimilarProductsRequest, GetSimilarProductsResult};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct SimilarProductsQuery {
    #[serde(default)]
    language: LanguageData,
    #[serde(default)]
    currency: CurrencyData,
}

const READY_CACHE_CONTROL: &str = "public, max-age=180, s-maxage=900";
const PENDING_CACHE_CONTROL: &str = "public, max-age=300, s-maxage=900";

pub async fn get_similar_products_by_id(
    State(state): State<ProductsState>,
    headers: HeaderMap,
    Path(raw_product_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let product_id = match ProductId::try_from(raw_product_id.as_str()) {
        Ok(value) => value,
        Err(_) => {
            return ApiError::bad_request(INVALID_UUID)
                .with_path_field("productId")
                .with_detail("Path parameter 'productId' must be a UUID.")
                .into_response();
        }
    };
    let query = match parse_query(raw_query.as_deref()) {
        Ok(query) => query,
        Err(error) => return error.into_response(),
    };
    similar_response(
        state,
        headers,
        ProductEmbeddingLookup::ById(product_id),
        query,
    )
    .await
}

pub async fn get_similar_products_by_slug(
    State(state): State<ProductsState>,
    headers: HeaderMap,
    Path((raw_shop_slug_id, raw_product_slug_id)): Path<(String, String)>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let shop_slug_id = match ShopSlugId::raw(&raw_shop_slug_id) {
        Ok(value) => value,
        Err(_) => {
            return ApiError::bad_request(BAD_PATH_PARAMETER_VALUE)
                .with_path_field("shopSlugId")
                .with_detail("Path parameter 'shopSlugId' is invalid.")
                .into_response();
        }
    };
    let product_slug_id = match ProductSlugId::raw(&raw_product_slug_id) {
        Ok(value) => value,
        Err(_) => {
            return ApiError::bad_request(BAD_PATH_PARAMETER_VALUE)
                .with_path_field("productSlugId")
                .with_detail("Path parameter 'productSlugId' is invalid.")
                .into_response();
        }
    };
    let query = match parse_query(raw_query.as_deref()) {
        Ok(query) => query,
        Err(error) => return error.into_response(),
    };
    similar_response(
        state,
        headers,
        ProductEmbeddingLookup::BySlug {
            shop_slug_id,
            product_slug_id,
        },
        query,
    )
    .await
}

fn parse_query(raw_query: Option<&str>) -> Result<SimilarProductsQuery, ApiError> {
    serde_qs::from_str(raw_query.unwrap_or_default()).map_err(|error| {
        ApiError::bad_request(crate::error::BAD_QUERY_PARAMETER_VALUE)
            .with_detail(error.to_string())
    })
}

async fn similar_response(
    state: ProductsState,
    headers: HeaderMap,
    lookup: ProductEmbeddingLookup,
    query: SimilarProductsQuery,
) -> Response {
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let context = principal.operation_context(metadata);
    match state
        .get_similar_products
        .execute(
            &context,
            GetSimilarProductsRequest {
                lookup: lookup.clone(),
                language: query.language.into(),
                currency: query.currency.into(),
            },
        )
        .await
    {
        Ok(GetSimilarProductsResult::Ready(products)) => ready_response(products, &principal),
        Ok(GetSimilarProductsResult::EmbeddingPending) => pending_response(lookup),
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn ready_response(
    products: Vec<product_service::use_cases::PersonalizedProductSummary>,
    principal: &crate::auth::TransportPrincipal,
) -> Response {
    let mut response = Json(
        products
            .into_iter()
            .map(personalized_product_summary_data)
            .collect::<Vec<_>>(),
    )
    .into_response();
    let cache_control = match principal {
        crate::auth::TransportPrincipal::Anonymous => READY_CACHE_CONTROL,
        crate::auth::TransportPrincipal::User { .. } => "no-store",
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    response
}

fn pending_response(lookup: ProductEmbeddingLookup) -> Response {
    let location_path = match lookup {
        ProductEmbeddingLookup::ById(product_id) => {
            format!("/api/v1/products/{product_id}/similar")
        }
        ProductEmbeddingLookup::BySlug {
            shop_slug_id,
            product_slug_id,
        } => format!("/api/v1/by-slug/shops/{shop_slug_id}/products/{product_slug_id}/similar"),
    };
    let location = match HeaderValue::from_str(&location_path) {
        Ok(value) => value,
        Err(_) => {
            return ApiError::internal_server_error(PRODUCT_INTERNAL_ERROR)
                .with_detail("Similar product polling location failed internally.")
                .into_response();
        }
    };
    let mut response = StatusCode::ACCEPTED.into_response();
    response.headers_mut().insert(header::LOCATION, location);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(PENDING_CACHE_CONTROL),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use common::currency::domain::Currency;
    use common::event_id::EventId;
    use common::language::domain::Language;
    use common::localized::Localized;
    use common::operation_context::OperationContext;
    use common::personalized::Personalized;
    use common::price::domain::{MonetaryAmount, Price};
    use common::product_lifecycle::domain::ProductLifecycle;
    use common::product_slug_id::ProductSlugId;
    use common::product_state::domain::ProductState;
    use common::shop_id::ShopId;
    use common::shop_name::ShopName;
    use common::shop_slug_id::ShopSlugId;
    use common::shops_product_id::ShopsProductId;
    use common::user_id::UserId;
    use product_core::title::Title;
    use product_core::user_state::ProductUserState;
    use product_service::use_cases::{
        GetProductError, GetProductRequest, GetProductUseCase, GetSimilarProductsError,
        GetSimilarProductsRequest, GetSimilarProductsResult, GetSimilarProductsUseCase,
        PersonalizedProductDetailsView, PersonalizedProductSummary, ProductSummary,
        ProductSummaryPriceValuation, SearchProductsError, SearchProductsRequest,
        SearchProductsResult, SearchProductsUseCase,
    };
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use time::OffsetDateTime;
    use tower::ServiceExt;
    use url::Url;

    #[derive(Clone)]
    enum FakeSimilarProductsResult {
        Ready(Vec<PersonalizedProductSummary>),
        Pending,
        NotFound,
        Unavailable,
    }

    struct FakeSimilarProductsUseCase {
        result: FakeSimilarProductsResult,
    }

    #[async_trait::async_trait]
    impl GetSimilarProductsUseCase for FakeSimilarProductsUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: GetSimilarProductsRequest,
        ) -> Result<GetSimilarProductsResult, GetSimilarProductsError> {
            match &self.result {
                FakeSimilarProductsResult::Ready(products) => {
                    Ok(GetSimilarProductsResult::Ready(products.clone()))
                }
                FakeSimilarProductsResult::Pending => {
                    Ok(GetSimilarProductsResult::EmbeddingPending)
                }
                FakeSimilarProductsResult::NotFound => Err(GetSimilarProductsError::NotFound),
                FakeSimilarProductsResult::Unavailable => {
                    Err(GetSimilarProductsError::SimilaritySearchUnavailable)
                }
            }
        }
    }

    struct UnusedGetProductUseCase;

    #[async_trait::async_trait]
    impl GetProductUseCase for UnusedGetProductUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: GetProductRequest,
        ) -> Result<PersonalizedProductDetailsView, GetProductError> {
            Err(GetProductError::NotFound)
        }
    }

    struct UnusedSearchProductsUseCase;

    #[async_trait::async_trait]
    impl SearchProductsUseCase for UnusedSearchProductsUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: SearchProductsRequest,
        ) -> Result<SearchProductsResult, SearchProductsError> {
            Ok(SearchProductsResult::default())
        }
    }

    struct FakeAuthenticator {
        principal: TransportPrincipal,
    }

    #[async_trait::async_trait]
    impl TokenAuthenticator for FakeAuthenticator {
        async fn authenticate(
            &self,
            _bearer_token: &str,
            _metadata: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            Ok(self.principal.clone())
        }
    }

    #[test]
    fn should_parse_similar_products_currency_and_language() {
        let query = parse_query(Some("language=de&currency=USD"));
        assert!(
            matches!(query, Ok(query) if query.language == LanguageData::De && query.currency == CurrencyData::Usd)
        );

        let default_query = parse_query(None);
        assert!(
            matches!(default_query, Ok(query) if query.language == LanguageData::En && query.currency == CurrencyData::Eur)
        );
    }

    #[test]
    fn should_reject_invalid_similar_products_query() {
        assert!(parse_query(Some("language=invalid")).is_err());
        assert!(parse_query(Some("currency=invalid")).is_err());
    }

    #[tokio::test]
    async fn should_return_ready_similar_products_as_json_with_public_cache_header()
    -> Result<(), Box<dyn std::error::Error>> {
        let product = product_summary()?;
        let product_id = product.item.product_id;
        let app = app(
            FakeSimilarProductsResult::Ready(vec![product]),
            TransportPrincipal::Anonymous,
        );

        let response = app
            .oneshot(
                Request::get(format!("/api/v1/products/{product_id}/similar"))
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            READY_CACHE_CONTROL,
            response.headers()[header::CACHE_CONTROL]
        );
        let body = body_json(response).await?;
        assert_eq!(product_id.to_string(), body[0]["item"]["productId"]);
        assert_eq!("Cabinet", body[0]["item"]["title"]["text"]);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_cache_ready_similar_products_for_authenticated_request()
    -> Result<(), Box<dyn std::error::Error>> {
        let product_id = ProductId::new();
        let mut product = product_summary()?;
        product.user_state = Some(ProductUserState::default());
        let app = app(
            FakeSimilarProductsResult::Ready(vec![product]),
            TransportPrincipal::User {
                user_id: UserId::new(),
                auth_method: AuthMethod::CognitoJwt,
                capabilities: BTreeSet::new(),
            },
        );

        let response = app
            .oneshot(
                Request::get(format!("/api/v1/products/{product_id}/similar"))
                    .header(header::AUTHORIZATION, "Bearer token")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
        assert_eq!(
            serde_json::json!([]),
            body_json(response).await?[0]["userState"]["notification"]["unseenNotificationIds"]
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_return_pending_response_with_id_location_and_cache_header()
    -> Result<(), Box<dyn std::error::Error>> {
        let product_id = ProductId::new();
        let app = app(
            FakeSimilarProductsResult::Pending,
            TransportPrincipal::Anonymous,
        );

        let response = app
            .oneshot(
                Request::get(format!("/api/v1/products/{product_id}/similar"))
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::ACCEPTED, response.status());
        assert_eq!(
            format!("/api/v1/products/{product_id}/similar"),
            response.headers()[header::LOCATION]
        );
        assert_eq!(
            PENDING_CACHE_CONTROL,
            response.headers()[header::CACHE_CONTROL]
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_return_pending_response_with_slug_location_and_cache_header()
    -> Result<(), Box<dyn std::error::Error>> {
        let shop_slug_id = "antique-depot";
        let product_slug_id = "cabinet-a1b2c3";
        let app = app(
            FakeSimilarProductsResult::Pending,
            TransportPrincipal::Anonymous,
        );

        let response = app
            .oneshot(
                Request::get(format!(
                    "/api/v1/by-slug/shops/{shop_slug_id}/products/{product_slug_id}/similar"
                ))
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::ACCEPTED, response.status());
        assert_eq!(
            format!("/api/v1/by-slug/shops/{shop_slug_id}/products/{product_slug_id}/similar"),
            response.headers()[header::LOCATION]
        );
        assert_eq!(
            PENDING_CACHE_CONTROL,
            response.headers()[header::CACHE_CONTROL]
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_map_similar_product_not_found_error() -> Result<(), Box<dyn std::error::Error>>
    {
        let app = app(
            FakeSimilarProductsResult::NotFound,
            TransportPrincipal::Anonymous,
        );

        let response = app
            .oneshot(
                Request::get(format!("/api/v1/products/{}/similar", ProductId::new()))
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::NOT_FOUND, response.status());
        assert_eq!("PRODUCT_NOT_FOUND", body_json(response).await?["error"]);
        Ok(())
    }

    #[tokio::test]
    async fn should_map_similarity_service_unavailable_error()
    -> Result<(), Box<dyn std::error::Error>> {
        let app = app(
            FakeSimilarProductsResult::Unavailable,
            TransportPrincipal::Anonymous,
        );

        let response = app
            .oneshot(
                Request::get(format!("/api/v1/products/{}/similar", ProductId::new()))
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::SERVICE_UNAVAILABLE, response.status());
        assert_eq!(
            "PRODUCT_TEMPORARILY_UNAVAILABLE",
            body_json(response).await?["error"]
        );
        Ok(())
    }

    fn app(result: FakeSimilarProductsResult, principal: TransportPrincipal) -> Router {
        let state = ProductsState::new(
            Arc::new(UnusedGetProductUseCase),
            Arc::new(FakeSimilarProductsUseCase { result }),
            Arc::new(UnusedSearchProductsUseCase),
            Arc::new(FakeAuthenticator { principal }),
        );
        Router::new()
            .route(
                "/api/v1/products/{product_id}/similar",
                axum::routing::get(get_similar_products_by_id),
            )
            .route(
                "/api/v1/by-slug/shops/{shop_slug_id}/products/{product_slug_id}/similar",
                axum::routing::get(get_similar_products_by_slug),
            )
            .with_state(state)
    }

    async fn body_json(
        response: Response,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn product_summary() -> Result<PersonalizedProductSummary, url::ParseError> {
        Ok(Personalized {
            item: ProductSummary {
                product_id: ProductId::new(),
                product_slug_id: ProductSlugId::from("cabinet-abcdef"),
                event_id: EventId::new(),
                shop_id: ShopId::new(),
                seller_id: ShopId::new(),
                shops_product_id: ShopsProductId::new(),
                shop_name: ShopName::from("Shop"),
                shop_slug_id: ShopSlugId::from("shop"),
                title: Some(Localized {
                    localization: Language::En,
                    payload: Title::from("Cabinet"),
                }),
                display_price: Some(Price::new(MonetaryAmount::from(100_u64), Currency::Eur)),
                price_valuation: ProductSummaryPriceValuation::Current {
                    fx_rate_id: common::fx_rate_id::FxRateId::new(),
                    captured_at: OffsetDateTime::UNIX_EPOCH,
                },
                state: ProductState::Listed,
                lifecycle: ProductLifecycle::Active,
                url: Url::parse("https://shop.example/products/1")?,
                view_url: Url::parse("https://aura.example/products/cabinet-abcdef")?,
                images: Default::default(),
                updated: OffsetDateTime::UNIX_EPOCH,
            },
            user_state: None,
        })
    }
}
