use super::util::no_store;
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_QUERY_PARAMETER_VALUE, WATCHLIST_INTERNAL_ERROR};
use crate::products::product_data::{
    PersonalizedProductDetailsData, personalized_product_details_data,
};
use crate::state::WatchlistState;
use crate::values::{CurrencyData, LanguageData};
use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use common::pagination::cursor::{Cursor, CursoredResult, api::JsonCursoredData};
use common::product_id::ProductId;
use product_service::ports::ProductWatchlistDetailsCursor;
use serde::Deserialize;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use watchlist_service::use_cases::ListWatchlistRequest;

#[derive(Debug, Deserialize)]
struct ListWatchlistQuery {
    #[serde(default)]
    language: LanguageData,
    #[serde(default)]
    currency: CurrencyData,
    size: Option<u64>,
    #[serde(rename = "searchAfter")]
    search_after: Option<String>,
}

pub async fn list_watchlist(
    State(state): State<WatchlistState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let query: ListWatchlistQuery =
        match serde_qs::from_str(raw_query.as_deref().unwrap_or_default()) {
            Ok(query) => query,
            Err(error) => {
                return ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                    .with_detail(error.to_string())
                    .into_response();
            }
        };
    let cursor = match watchlist_cursor(query.size, query.search_after.as_deref()) {
        Ok(cursor) => cursor,
        Err(error) => return error.into_response(),
    };
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    match state
        .list_watchlist
        .execute(
            &ctx,
            ListWatchlistRequest {
                user_id,
                language: query.language.into(),
                currency: query.currency.into(),
                cursor,
            },
        )
        .await
    {
        Ok(result) => {
            let search_after = match result
                .cursor
                .search_after
                .map(watchlist_cursor_value)
                .transpose()
            {
                Ok(search_after) => search_after,
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

fn watchlist_cursor(
    size: Option<u64>,
    search_after: Option<&str>,
) -> Result<Cursor<ProductWatchlistDetailsCursor>, ApiError> {
    let size = size.unwrap_or(21).clamp(1, 100);
    let search_after = search_after
        .map(|value| {
            serde_json::from_str::<Value>(value)
                .map_err(|error| {
                    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                        .with_query_field("searchAfter")
                        .with_detail(error.to_string())
                })
                .and_then(parse_watchlist_cursor)
        })
        .transpose()?;
    Ok(Cursor { size, search_after })
}

fn parse_watchlist_cursor(value: Value) -> Result<ProductWatchlistDetailsCursor, ApiError> {
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
    let watchlist_created = OffsetDateTime::parse(created, &Rfc3339).map_err(|error| {
        ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            .with_query_field("searchAfter")
            .with_detail(error.to_string())
    })?;
    let product_id = ProductId::try_from(product_id).map_err(|_| {
        ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            .with_query_field("searchAfter")
            .with_detail("searchAfter must contain a product UUID.")
    })?;
    Ok(ProductWatchlistDetailsCursor {
        watchlist_created,
        product_id,
    })
}

fn watchlist_cursor_value(cursor: ProductWatchlistDetailsCursor) -> Result<Value, ApiError> {
    cursor
        .watchlist_created
        .format(&Rfc3339)
        .map(|created| json!([created, cursor.product_id]))
        .map_err(|_| {
            ApiError::internal_server_error(WATCHLIST_INTERNAL_ERROR)
                .with_detail("Watchlist cursor failed internally.")
        })
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
    use common::event_id::EventId;
    use common::fx_rate_id::FxRateId;
    use common::operation_context::OperationContext;
    use common::personalized::Personalized;
    use common::product_id::ProductId;
    use common::product_lifecycle::domain::ProductLifecycle;
    use common::product_slug_id::ProductSlugId;
    use common::product_state::domain::ProductState;
    use common::shop_id::ShopId;
    use common::shop_name::ShopName;
    use common::shop_slug_id::ShopSlugId;
    use common::shops_product_id::ShopsProductId;
    use common::user_id::UserId;
    use localization::Language;
    use product_core::product::{ProductAddress, ProductAuction, ProductPricing};
    use product_core::user_state::ProductUserState;
    use std::sync::{Arc, Mutex, MutexGuard};
    use time::OffsetDateTime;
    use tower::ServiceExt;
    use url::Url;
    use watchlist_service::use_cases::{
        ListWatchlistError, ListWatchlistResult, ListWatchlistUseCase, UnwatchProductCommand,
        UnwatchProductError, UnwatchProductUseCase, UpdateWatchlistProductCommand,
        UpdateWatchlistProductError, UpdateWatchlistProductUseCase, WatchProductCommand,
        WatchProductError, WatchProductUseCase,
    };
    use watchlist_service::use_cases::{
        UnwatchProductResult, UpdateWatchlistProductResult, WatchProductResult,
    };

    type ListRequests = Arc<Mutex<Vec<(OperationContext, ListWatchlistRequest)>>>;

    #[derive(Clone)]
    struct FakeListWatchlistUseCase {
        result: ListWatchlistResult,
        requests: ListRequests,
    }

    #[async_trait::async_trait]
    impl ListWatchlistUseCase for FakeListWatchlistUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            request: ListWatchlistRequest,
        ) -> Result<ListWatchlistResult, ListWatchlistError> {
            lock(&self.requests).push((context.clone(), request));
            Ok(self.result.clone())
        }
    }

    struct UnusedWatchProductUseCase;
    #[async_trait::async_trait]
    impl WatchProductUseCase for UnusedWatchProductUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _command: WatchProductCommand,
        ) -> Result<WatchProductResult, WatchProductError> {
            Err(WatchProductError::TemporarilyUnavailable)
        }
    }

    struct UnusedUpdateWatchlistProductUseCase;
    #[async_trait::async_trait]
    impl UpdateWatchlistProductUseCase for UnusedUpdateWatchlistProductUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _command: UpdateWatchlistProductCommand,
        ) -> Result<UpdateWatchlistProductResult, UpdateWatchlistProductError> {
            Err(UpdateWatchlistProductError::TemporarilyUnavailable)
        }
    }

    struct UnusedUnwatchProductUseCase;
    #[async_trait::async_trait]
    impl UnwatchProductUseCase for UnusedUnwatchProductUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _command: UnwatchProductCommand,
        ) -> Result<UnwatchProductResult, UnwatchProductError> {
            Err(UnwatchProductError::TemporarilyUnavailable)
        }
    }

    struct FakeAuthenticator {
        user_id: UserId,
        reject: bool,
    }

    #[async_trait::async_trait]
    impl TokenAuthenticator for FakeAuthenticator {
        async fn authenticate(
            &self,
            _bearer_token: &str,
            _metadata: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            if self.reject {
                Err(AuthError::InvalidCredentials)
            } else {
                Ok(TransportPrincipal::User {
                    user_id: self.user_id,
                    auth_method: AuthMethod::CognitoJwt,
                    capabilities: Default::default(),
                })
            }
        }
    }

    fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
        match value.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn product(
        product_id: ProductId,
    ) -> Result<product_service::use_cases::PersonalizedProductDetailsView, url::ParseError> {
        let url = Url::parse("https://example.test/product")?;
        Ok(Personalized {
            item: product_service::use_cases::ProductDetailsView {
                product_id,
                product_slug_id: ProductSlugId::from("product"),
                event_id: EventId::new(),
                shop_id: ShopId::new(),
                seller_id: ShopId::new(),
                shops_product_id: ShopsProductId::from("product"),
                shop_name: ShopName::from("Shop"),
                seller_name: ShopName::from("Seller"),
                shop_slug_id: ShopSlugId::from("shop"),
                seller_slug_id: ShopSlugId::from("seller"),
                address: ProductAddress::default(),
                product_title: None,
                product_description: None,
                title: None,
                description: None,
                pricing: product_service::use_cases::ProductPricingPresentation {
                    source: ProductPricing::default(),
                    display: product_service::use_cases::DisplayProductPricing {
                        price: None,
                        price_estimate_min: None,
                        price_estimate_max: None,
                    },
                    valuation: product_service::use_cases::ProductPricingValuation::Current {
                        fx_rate_id: FxRateId::new(),
                        captured_at: OffsetDateTime::UNIX_EPOCH,
                    },
                },
                state: ProductState::Available,
                lifecycle: ProductLifecycle::Active,
                url: url.clone(),
                view_url: url,
                images: Default::default(),
                auction: ProductAuction::default(),
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            },
            user_state: Some(ProductUserState::default()),
        })
    }

    fn app(
        user_id: UserId,
        products: Vec<product_service::use_cases::PersonalizedProductDetailsView>,
        reject_auth: bool,
    ) -> (Router, ListRequests) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let state = WatchlistState {
            list_watchlist: Arc::new(FakeListWatchlistUseCase {
                result: ListWatchlistResult {
                    items: products,
                    cursor: Cursor::default(),
                    total: None,
                },
                requests: Arc::clone(&requests),
            }),
            watch_product: Arc::new(UnusedWatchProductUseCase),
            update_watchlist_product: Arc::new(UnusedUpdateWatchlistProductUseCase),
            unwatch_product: Arc::new(UnusedUnwatchProductUseCase),
            authenticator: Arc::new(FakeAuthenticator {
                user_id,
                reject: reject_auth,
            }),
        };
        (
            Router::new()
                .route("/api/v1/me/watchlist", axum::routing::get(list_watchlist))
                .with_state(state),
            requests,
        )
    }

    async fn json(response: Response) -> Result<serde_json::Value, axum::Error> {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        Ok(serde_json::from_slice(&bytes).unwrap_or_default())
    }

    #[tokio::test]
    async fn should_return_personalized_products_without_cache_and_forward_language()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let product_id = ProductId::new();
        let (app, requests) = app(user_id, vec![product(product_id)?], false);

        let response = app
            .oneshot(
                Request::get("/api/v1/me/watchlist?language=de")
                    .header(header::AUTHORIZATION, "Bearer valid")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
        let body = json(response).await?;
        assert_eq!(21, body["size"]);
        assert!(body["searchAfter"].is_null());
        assert_eq!(
            product_id.to_string(),
            body["items"][0]["item"]["productId"]
        );
        assert_eq!(true, body["items"][0]["userState"]["notification"]["seen"]);
        assert!(matches!(
            lock(&requests)[0].1,
            ListWatchlistRequest {
                user_id: actual_user_id,
                language: Language::De,
                currency: money::Currency::Eur,
                cursor: Cursor { size: 21, search_after: None },
            } if actual_user_id == user_id
        ));
        Ok(())
    }

    #[test]
    fn should_parse_tie_safe_watchlist_cursor_and_clamp_size()
    -> Result<(), Box<dyn std::error::Error>> {
        let product_id = ProductId::new();
        let raw_cursor = serde_json::to_string(&json!(["1970-01-01T00:00:00Z", product_id]))?;
        let cursor = watchlist_cursor(Some(500), Some(&raw_cursor))?;

        assert_eq!(100, cursor.size);
        let search_after = cursor.search_after.ok_or("cursor was missing")?;
        assert_eq!(product_id, search_after.product_id);
        assert_eq!(
            search_after,
            parse_watchlist_cursor(watchlist_cursor_value(search_after)?)?
        );
        assert!(watchlist_cursor(Some(1), Some("invalid")).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_language_before_authentication()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let (app, requests) = app(user_id, Vec::new(), false);

        let response = app
            .oneshot(Request::get("/api/v1/me/watchlist?language=invalid").body(Body::empty())?)
            .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!("BAD_QUERY_PARAMETER_VALUE", json(response).await?["error"]);
        assert!(lock(&requests).is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_bearer_token() -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let (app, requests) = app(user_id, Vec::new(), true);

        let response = app
            .oneshot(
                Request::get("/api/v1/me/watchlist")
                    .header(header::AUTHORIZATION, "Bearer invalid")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::UNAUTHORIZED, response.status());
        assert!(lock(&requests).is_empty());
        Ok(())
    }
}
