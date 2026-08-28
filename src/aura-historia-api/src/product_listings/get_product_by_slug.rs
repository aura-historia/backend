use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{ApiError, BAD_PATH_PARAMETER_VALUE, BAD_QUERY_PARAMETER_VALUE};
use crate::product_listings::product_data::product_response;
use crate::state::ProductListingsState;
use axum::extract::{Path, RawQuery, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use listing_source_core::ListingSourceSlugId;
use localization::Language;
use money::Currency;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_service::use_cases::{GetProductListingRequest, ProductListingLookup};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct ProductListingDetailsQuery {
    #[serde(default)]
    #[serde(with = "crate::wire::language")]
    language: Language,
    #[serde(default, with = "crate::wire::currency")]
    currency: Currency,
}

pub async fn get_product_by_slug(
    State(state): State<ProductListingsState>,
    headers: HeaderMap,
    Path((raw_shop_slug_id, raw_product_listing_slug_id)): Path<(String, String)>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let query: ProductListingDetailsQuery =
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

    let listing_source_slug_id = match ListingSourceSlugId::raw(&raw_shop_slug_id) {
        Ok(listing_source_slug_id) => listing_source_slug_id,
        Err(_) => {
            return ApiError::bad_request(BAD_PATH_PARAMETER_VALUE)
                .with_path_field("shopSlugId")
                .with_detail("Path parameter 'shopSlugId' is invalid.")
                .into_response();
        }
    };
    let product_listing_slug_id = match ProductListingSlugId::raw(&raw_product_listing_slug_id) {
        Ok(product_listing_slug_id) => product_listing_slug_id,
        Err(_) => {
            return ApiError::bad_request(BAD_PATH_PARAMETER_VALUE)
                .with_path_field("productListingSlugId")
                .with_detail("Path parameter 'productListingSlugId' is invalid.")
                .into_response();
        }
    };

    let context = principal.operation_context(metadata);
    match state
        .get_product
        .execute(
            &context,
            GetProductListingRequest {
                lookup: ProductListingLookup::BySlug {
                    listing_source_slug_id,
                    product_listing_slug_id,
                },
                language: query.language,
                currency: query.currency,
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
    use application::operation_context::OperationContext;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use localization::Language;
    use product_listing_service::use_cases::{
        GetProductListingError, GetProductListingUseCase, GetSimilarProductListingsError,
        GetSimilarProductListingsRequest, GetSimilarProductListingsResult,
        GetSimilarProductListingsUseCase, SearchProductListingsError, SearchProductListingsRequest,
        SearchProductListingsResult, SearchProductListingsUseCase,
    };
    use std::sync::{Arc, Mutex, MutexGuard};
    use tower::ServiceExt;

    type GetProductListingCalls = Arc<Mutex<Vec<GetProductListingRequest>>>;

    struct FakeGetProductListingUseCase {
        calls: GetProductListingCalls,
    }

    #[async_trait::async_trait]
    impl GetProductListingUseCase for FakeGetProductListingUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            request: GetProductListingRequest,
        ) -> Result<
            product_listing_service::use_cases::PersonalizedProductListingDetailsView,
            GetProductListingError,
        > {
            lock(&self.calls).push(request);
            Err(GetProductListingError::NotFound)
        }
    }

    struct UnusedSimilarProductListingsUseCase;

    #[async_trait::async_trait]
    impl GetSimilarProductListingsUseCase for UnusedSimilarProductListingsUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: GetSimilarProductListingsRequest,
        ) -> Result<GetSimilarProductListingsResult, GetSimilarProductListingsError> {
            Err(GetSimilarProductListingsError::SimilaritySearchUnavailable)
        }
    }

    struct UnusedSearchProductListingsUseCase;

    #[async_trait::async_trait]
    impl SearchProductListingsUseCase for UnusedSearchProductListingsUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: SearchProductListingsRequest,
        ) -> Result<SearchProductListingsResult, SearchProductListingsError> {
            Ok(SearchProductListingsResult::default())
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
        let raw_product_listing_slug_id = "louis-xvi-commode-a1b2c3";
        let (app, calls) = app();

        let response = app
            .oneshot(
                Request::get(format!(
                    "/api/v1/by-slug/shops/{raw_shop_slug_id}/product-listings/{raw_product_listing_slug_id}"
                ))
                .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::NOT_FOUND, response.status());
        assert!(matches!(
            lock(&calls).as_slice(),
            [GetProductListingRequest {
                lookup: ProductListingLookup::BySlug {
                    listing_source_slug_id,
                    product_listing_slug_id,
                },
                language: Language::En,
                currency: money::Currency::Eur,
            }] if listing_source_slug_id.as_ref() == raw_shop_slug_id
                && product_listing_slug_id.as_ref() == raw_product_listing_slug_id
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_pass_requested_language_for_slug_lookup()
    -> Result<(), Box<dyn std::error::Error>> {
        let (app, calls) = app();

        let response = app
            .oneshot(
                Request::get("/api/v1/by-slug/shops/antique-depot/product-listings/louis-xvi-commode-a1b2c3?language=de&currency=USD")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::NOT_FOUND, response.status());
        assert!(matches!(
            lock(&calls).as_slice(),
            [GetProductListingRequest {
                lookup: ProductListingLookup::BySlug { .. },
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
                Request::get("/api/v1/by-slug/shops/antique-depot/product-listings/commode-A1B2C3")
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
                Request::get("/api/v1/by-slug/shops/Antique-Depot/product-listings/commode-a1b2c3")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert!(lock(&calls).is_empty());
        Ok(())
    }

    fn app() -> (Router, GetProductListingCalls) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let state = ProductListingsState::new(
            Arc::new(FakeGetProductListingUseCase {
                calls: Arc::clone(&calls),
            }),
            Arc::new(UnusedSimilarProductListingsUseCase),
            Arc::new(UnusedSearchProductListingsUseCase),
            Arc::new(AnonymousAuthenticator),
        );
        (
            Router::new()
                .route(
                    "/api/v1/by-slug/shops/{shop_slug_id}/product-listings/{product_listing_slug_id}",
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
