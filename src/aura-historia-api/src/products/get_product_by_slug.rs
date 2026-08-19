use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{ApiError, BAD_PATH_PARAMETER_VALUE, BAD_QUERY_PARAMETER_VALUE};
use crate::products::product_data::product_response;
use crate::state::ProductsState;
use crate::values::{CurrencyData, LanguageData};
use axum::extract::{Path, RawQuery, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use common::product_slug_id::ProductSlugId;
use product_service::use_cases::{GetProductRequest, ProductLookup};
use serde::Deserialize;
use shop_core::shop_slug_id::ShopSlugId;

#[derive(Debug, Deserialize)]
struct ProductDetailsQuery {
    #[serde(default)]
    language: LanguageData,
    #[serde(default)]
    currency: CurrencyData,
}

pub async fn get_product_by_slug(
    State(state): State<ProductsState>,
    headers: HeaderMap,
    Path((raw_shop_slug_id, raw_product_slug_id)): Path<(String, String)>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let query: ProductDetailsQuery =
        match serde_qs::from_str(raw_query.as_deref().unwrap_or_default()) {
            Ok(query) => query,
            Err(error) => {
                return ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                    .with_detail(error.to_string())
                    .into_response();
            }
        };
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return ApiError::from(error).into_response(),
    };

    let shop_slug_id = match ShopSlugId::raw(&raw_shop_slug_id) {
        Ok(shop_slug_id) => shop_slug_id,
        Err(_) => {
            return ApiError::bad_request(BAD_PATH_PARAMETER_VALUE)
                .with_path_field("shopSlugId")
                .with_detail("Path parameter 'shopSlugId' is invalid.")
                .into_response();
        }
    };
    let product_slug_id = match ProductSlugId::raw(&raw_product_slug_id) {
        Ok(product_slug_id) => product_slug_id,
        Err(_) => {
            return ApiError::bad_request(BAD_PATH_PARAMETER_VALUE)
                .with_path_field("productSlugId")
                .with_detail("Path parameter 'productSlugId' is invalid.")
                .into_response();
        }
    };

    let context = principal.operation_context(metadata);
    match state
        .get_product
        .execute(
            &context,
            GetProductRequest {
                lookup: ProductLookup::BySlug {
                    shop_slug_id,
                    product_slug_id,
                },
                language: query.language.into(),
                currency: query.currency.into(),
            },
        )
        .await
    {
        Ok(view) => product_response(view, &context.principal),
        Err(error) => ApiError::from(error).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthError, RequestMetadata, TokenAuthenticator, TransportPrincipal};
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use common::operation_context::OperationContext;
    use localization::Language;
    use product_service::use_cases::{
        GetProductError, GetProductUseCase, GetSimilarProductsError, GetSimilarProductsRequest,
        GetSimilarProductsResult, GetSimilarProductsUseCase, SearchProductsError,
        SearchProductsRequest, SearchProductsResult, SearchProductsUseCase,
    };
    use std::sync::{Arc, Mutex, MutexGuard};
    use tower::ServiceExt;

    type GetProductCalls = Arc<Mutex<Vec<GetProductRequest>>>;

    struct FakeGetProductUseCase {
        calls: GetProductCalls,
    }

    #[async_trait::async_trait]
    impl GetProductUseCase for FakeGetProductUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            request: GetProductRequest,
        ) -> Result<product_service::use_cases::PersonalizedProductDetailsView, GetProductError>
        {
            lock(&self.calls).push(request);
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
    async fn should_pass_valid_raw_slug_path_values_unchanged_to_use_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let raw_shop_slug_id = "antique-depot";
        let raw_product_slug_id = "louis-xvi-commode-a1b2c3";
        let (app, calls) = app();

        let response = app
            .oneshot(
                Request::get(format!(
                    "/api/v1/by-slug/shops/{raw_shop_slug_id}/products/{raw_product_slug_id}"
                ))
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::NOT_FOUND, response.status());
        assert!(matches!(
            lock(&calls).as_slice(),
            [GetProductRequest {
                lookup: ProductLookup::BySlug {
                    shop_slug_id,
                    product_slug_id,
                },
                language: Language::En,
                currency: money::Currency::Eur,
            }] if shop_slug_id.as_ref() == raw_shop_slug_id
                && product_slug_id.as_ref() == raw_product_slug_id
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_pass_requested_language_for_slug_lookup()
    -> Result<(), Box<dyn std::error::Error>> {
        let (app, calls) = app();

        let response = app
            .oneshot(
                Request::get("/api/v1/by-slug/shops/antique-depot/products/louis-xvi-commode-a1b2c3?language=de&currency=USD")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::NOT_FOUND, response.status());
        assert!(matches!(
            lock(&calls).as_slice(),
            [GetProductRequest {
                lookup: ProductLookup::BySlug { .. },
                language: Language::De,
                currency: money::Currency::Usd,
            }]
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_malformed_product_slug_before_calling_use_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let (app, calls) = app();

        let response = app
            .oneshot(
                Request::get("/api/v1/by-slug/shops/antique-depot/products/commode-A1B2C3")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert!(lock(&calls).is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_malformed_shop_slug_before_calling_use_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let (app, calls) = app();

        let response = app
            .oneshot(
                Request::get("/api/v1/by-slug/shops/Antique-Depot/products/commode-a1b2c3")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert!(lock(&calls).is_empty());
        Ok(())
    }

    fn app() -> (Router, GetProductCalls) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let state = ProductsState::new(
            Arc::new(FakeGetProductUseCase {
                calls: Arc::clone(&calls),
            }),
            Arc::new(UnusedSimilarProductsUseCase),
            Arc::new(UnusedSearchProductsUseCase),
            Arc::new(AnonymousAuthenticator),
        );
        (
            Router::new()
                .route(
                    "/api/v1/by-slug/shops/{shop_slug_id}/products/{product_slug_id}",
                    axum::routing::get(get_product_by_slug),
                )
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
