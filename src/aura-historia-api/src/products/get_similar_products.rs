use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{ApiError, BAD_PATH_PARAMETER_VALUE, INVALID_UUID, PRODUCT_INTERNAL_ERROR};
use crate::state::ProductsState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use common::product_id::ProductKey;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use product_service::use_cases::{GetSimilarProductsRequest, GetSimilarProductsResult};

const PENDING_CACHE_CONTROL: &str = "public, max-age=300, s-maxage=900";

pub async fn get_similar_products(
    State(state): State<ProductsState>,
    headers: HeaderMap,
    Path((raw_shop_id, raw_shops_product_id)): Path<(String, String)>,
) -> Response {
    let metadata = request_metadata(&headers);
    if let Err(error) = OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        return ApiError::from(error).into_response();
    }

    let shop_id = match ShopId::try_from(raw_shop_id.as_str()) {
        Ok(shop_id) => shop_id,
        Err(_) => {
            return ApiError::bad_request(INVALID_UUID)
                .with_path_field("shopId")
                .with_detail("Path parameter 'shopId' must be a UUID.")
                .into_response();
        }
    };
    let shops_product_id = match ShopsProductId::raw(&raw_shops_product_id) {
        Ok(shops_product_id) => shops_product_id,
        Err(_) => {
            return ApiError::bad_request(BAD_PATH_PARAMETER_VALUE)
                .with_path_field("shopsProductId")
                .with_detail("Path parameter 'shopsProductId' is invalid.")
                .into_response();
        }
    };
    let product_key = ProductKey::new(shop_id, shops_product_id.clone());

    match state
        .get_similar_products
        .execute(GetSimilarProductsRequest { product_key })
        .await
    {
        Ok(GetSimilarProductsResult::EmbeddingPending) => {
            pending_response(shop_id, shops_product_id)
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn pending_response(shop_id: ShopId, shops_product_id: ShopsProductId) -> Response {
    let location = format!("/api/v1/shops/{shop_id}/products/{shops_product_id}/similar");
    let location = match HeaderValue::from_str(&location) {
        Ok(location) => location,
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
    use crate::auth::{AuthError, RequestMetadata, TokenAuthenticator, TransportPrincipal};
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use product_service::use_cases::{
        GetProductError, GetProductRequest, GetProductUseCase, ProductDetailsView,
        SearchProductsError, SearchProductsRequest, SearchProductsResult, SearchProductsUseCase,
    };
    use std::sync::{Arc, Mutex, MutexGuard};
    use tower::ServiceExt;

    type SimilarCalls = Arc<Mutex<Vec<GetSimilarProductsRequest>>>;

    struct FakeSimilarProductsUseCase {
        result: Mutex<
            Option<
                Result<
                    GetSimilarProductsResult,
                    product_service::use_cases::GetSimilarProductsError,
                >,
            >,
        >,
        calls: SimilarCalls,
    }

    #[async_trait::async_trait]
    impl product_service::use_cases::GetSimilarProductsUseCase for FakeSimilarProductsUseCase {
        async fn execute(
            &self,
            request: GetSimilarProductsRequest,
        ) -> Result<GetSimilarProductsResult, product_service::use_cases::GetSimilarProductsError>
        {
            lock(&self.calls).push(request);
            match lock(&self.result).take() {
                Some(result) => result,
                None => Err(product_service::use_cases::GetSimilarProductsError::SimilaritySearchUnavailable),
            }
        }
    }

    struct UnusedGetProductUseCase;

    #[async_trait::async_trait]
    impl GetProductUseCase for UnusedGetProductUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _request: GetProductRequest,
        ) -> Result<ProductDetailsView, GetProductError> {
            Err(GetProductError::NotFound)
        }
    }

    struct UnusedSearchProductsUseCase;

    #[async_trait::async_trait]
    impl SearchProductsUseCase for UnusedSearchProductsUseCase {
        async fn execute(
            &self,
            _context: &common::operation_context::OperationContext,
            _request: SearchProductsRequest,
        ) -> Result<SearchProductsResult, SearchProductsError> {
            Ok(SearchProductsResult::default())
        }
    }

    struct UnusedAuthenticator;

    #[async_trait::async_trait]
    impl TokenAuthenticator for UnusedAuthenticator {
        async fn authenticate(
            &self,
            _bearer_token: &str,
            _metadata: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            Err(AuthError::InvalidCredentials)
        }
    }

    #[tokio::test]
    async fn should_return_pending_with_shared_cache_and_relative_location()
    -> Result<(), Box<dyn std::error::Error>> {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let (app, calls) = app(Ok(GetSimilarProductsResult::EmbeddingPending));

        let response = app
            .oneshot(
                Request::get(format!(
                    "/api/v1/shops/{shop_id}/products/{shops_product_id}/similar"
                ))
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::ACCEPTED, response.status());
        assert_eq!(
            PENDING_CACHE_CONTROL,
            response.headers()[header::CACHE_CONTROL]
        );
        assert_eq!(
            format!("/api/v1/shops/{shop_id}/products/{shops_product_id}/similar"),
            response.headers()[header::LOCATION]
        );
        assert_eq!(
            vec![GetSimilarProductsRequest {
                product_key: ProductKey::new(shop_id, shops_product_id),
            }],
            *lock(&calls)
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_map_not_found_from_similar_products_use_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let (app, calls) = app(Err(
            product_service::use_cases::GetSimilarProductsError::NotFound,
        ));

        let response = app
            .oneshot(
                Request::get(format!(
                    "/api/v1/shops/{shop_id}/products/{shops_product_id}/similar"
                ))
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::NOT_FOUND, response.status());
        assert_eq!(1, lock(&calls).len());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_bearer_token_before_calling_similar_products_use_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let (app, calls) = app(Ok(GetSimilarProductsResult::EmbeddingPending));

        let response = app
            .oneshot(
                Request::get(format!(
                    "/api/v1/shops/{shop_id}/products/{shops_product_id}/similar"
                ))
                .header(header::AUTHORIZATION, "Bearer invalid")
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::UNAUTHORIZED, response.status());
        assert!(lock(&calls).is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_shop_id_before_calling_similar_products_use_case()
    -> Result<(), Box<dyn std::error::Error>> {
        let (app, calls) = app(Ok(GetSimilarProductsResult::EmbeddingPending));

        let response = app
            .oneshot(
                Request::get("/api/v1/shops/not-a-uuid/products/product/similar")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert!(lock(&calls).is_empty());
        Ok(())
    }

    fn app(
        result: Result<
            GetSimilarProductsResult,
            product_service::use_cases::GetSimilarProductsError,
        >,
    ) -> (Router, SimilarCalls) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let state = ProductsState::new(
            Arc::new(UnusedGetProductUseCase),
            Arc::new(FakeSimilarProductsUseCase {
                result: Mutex::new(Some(result)),
                calls: Arc::clone(&calls),
            }),
            Arc::new(UnusedSearchProductsUseCase),
            Arc::new(UnusedAuthenticator),
        );
        (
            Router::new()
                .route(
                    "/api/v1/shops/{shop_id}/products/{shops_product_id}/similar",
                    axum::routing::get(get_similar_products),
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
