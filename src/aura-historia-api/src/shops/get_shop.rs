use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{ApiError, INVALID_UUID};
use crate::shops::shop_data::{cache_control, shop_response};
use crate::state::ShopsState;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use shop_core::shop_id::ShopId;
use shop_service::use_cases::queries::get_shop::GetShopRequest;

pub async fn get_shop(
    State(state): State<ShopsState>,
    headers: HeaderMap,
    Path(raw_shop_id): Path<String>,
) -> Response {
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return ApiError::from(error).into_response(),
    };

    let shop_id = match ShopId::try_from(raw_shop_id.as_str()) {
        Ok(shop_id) => shop_id,
        Err(_) => {
            return ApiError::bad_request(INVALID_UUID)
                .with_path_field("shopId")
                .with_detail("Path parameter 'shopId' must be a UUID.")
                .into_response();
        }
    };

    let context = principal.operation_context(metadata);
    let cache_control = cache_control(&context.principal);
    match state
        .get_shop
        .execute(&context, GetShopRequest::ById(shop_id))
        .await
    {
        Ok(view) => shop_response(view, Some(cache_control)),
        Err(error) => ApiError::from(error).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use crate::error::SHOP_NOT_FOUND;
    use application::operation_context::{CredentialCapability, OperationContext, Principal};
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use localization::Language;
    use money::Currency;
    use serde_json::{Value, json};
    use shop_core::domain::Domain;
    use shop_core::partner_status::ShopPartnerStatus;
    use shop_core::shop_name::ShopName;
    use shop_core::shop_slug_id::ShopSlugId;
    use shop_core::shop_type::ShopType;
    use shop_service::use_cases::queries::get_shop::{GetShopError, ShopDetailsView};
    use std::collections::{BTreeSet, HashSet};
    use std::sync::{Arc, Mutex, MutexGuard};
    use time::macros::datetime;
    use tower::ServiceExt;
    use url::Url;
    use user_core::user_id::UserId;

    type GetShopCalls = Arc<Mutex<Vec<(OperationContext, GetShopRequest)>>>;

    #[derive(Clone)]
    struct FakeGetShopUseCase {
        result: Arc<Mutex<Result<ShopDetailsView, FakeGetShopError>>>,
        calls: GetShopCalls,
    }

    #[derive(Clone)]
    enum FakeGetShopError {
        NotFound,
    }

    #[async_trait::async_trait]
    impl shop_service::use_cases::queries::get_shop::GetShopUseCase for FakeGetShopUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            request: GetShopRequest,
        ) -> Result<ShopDetailsView, GetShopError> {
            lock(&self.calls).push((context.clone(), request));
            lock(&self.result).clone().map_err(|error| match error {
                FakeGetShopError::NotFound => GetShopError::NotFound,
            })
        }
    }

    #[derive(Clone)]
    struct FakeAuthenticator {
        result: Arc<Mutex<FakeAuthResult>>,
    }

    #[derive(Clone)]
    enum FakeAuthResult {
        Ok(TransportPrincipal),
        InvalidCredentials,
    }

    #[async_trait::async_trait]
    impl TokenAuthenticator for FakeAuthenticator {
        async fn authenticate(
            &self,
            _bearer_token: &str,
            _metadata: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            match lock(&self.result).clone() {
                FakeAuthResult::Ok(principal) => Ok(principal),
                FakeAuthResult::InvalidCredentials => Err(AuthError::InvalidCredentials),
            }
        }
    }

    #[tokio::test]
    async fn should_return_shop_for_anonymous_request() -> Result<(), Box<dyn std::error::Error>> {
        let shop_id = ShopId::new();
        let (app, calls) = app(
            Ok(view(shop_id)),
            FakeAuthResult::Ok(TransportPrincipal::Anonymous),
        );

        let response = app
            .oneshot(Request::get(format!("/api/v1/shops/{shop_id}")).body(Body::empty())?)
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            "public, max-age=600, s-maxage=3600",
            response.headers()[header::CACHE_CONTROL]
        );
        assert_eq!(
            "Thu, 01 Jan 1970 00:00:02 GMT",
            response.headers()[header::LAST_MODIFIED]
        );
        let body = body_json(response).await?;
        assert_eq!(json!(shop_id.to_string()), body["shopId"]);
        assert_eq!(json!("antik-markt"), body["shopSlugId"]);
        assert_eq!(json!("COMMERCIAL_DEALER"), body["shopType"]);
        assert_eq!(json!("EUR"), body["woocommerceCurrency"]);
        assert_eq!(json!("de"), body["woocommerceLanguage"]);
        assert!(body.get("createdBy").is_none());
        assert!(body.get("updatedBy").is_none());
        assert!(matches!(lock(&calls)[0].1, GetShopRequest::ById(actual) if actual == shop_id));
        Ok(())
    }

    #[tokio::test]
    async fn should_return_no_store_for_authenticated_request()
    -> Result<(), Box<dyn std::error::Error>> {
        let shop_id = ShopId::new();
        let user_id = UserId::new();
        let principal = TransportPrincipal::User {
            user_id,
            auth_method: AuthMethod::AuraAccessToken,
            capabilities: BTreeSet::from([CredentialCapability::ShopsRead]),
        };
        let (app, calls) = app(Ok(view(shop_id)), FakeAuthResult::Ok(principal));

        let response = app
            .oneshot(
                Request::get(format!("/api/v1/shops/{shop_id}"))
                    .header(
                        header::AUTHORIZATION,
                        "Bearer aurahistoria_accesstoken_good",
                    )
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::OK, response.status());
        assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
        assert!(
            matches!(lock(&calls)[0].0.principal, Principal::DelegatedUser { user_id: actual, .. } if actual == user_id)
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_uuid_before_use_case() -> Result<(), Box<dyn std::error::Error>>
    {
        let (app, calls) = app(
            Ok(view(ShopId::new())),
            FakeAuthResult::Ok(TransportPrincipal::Anonymous),
        );

        let response = app
            .oneshot(Request::get("/api/v1/shops/nope").body(Body::empty())?)
            .await?;

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert!(lock(&calls).is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_invalid_token_before_use_case() -> Result<(), Box<dyn std::error::Error>>
    {
        let shop_id = ShopId::new();
        let (app, calls) = app(Ok(view(shop_id)), FakeAuthResult::InvalidCredentials);

        let response = app
            .oneshot(
                Request::get(format!("/api/v1/shops/{shop_id}"))
                    .header(header::AUTHORIZATION, "Bearer bad")
                    .body(Body::empty())?,
            )
            .await?;

        assert_eq!(StatusCode::UNAUTHORIZED, response.status());
        assert!(lock(&calls).is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_return_not_found_when_use_case_returns_not_found()
    -> Result<(), Box<dyn std::error::Error>> {
        let shop_id = ShopId::new();
        let (app, _calls) = app(
            Err(FakeGetShopError::NotFound),
            FakeAuthResult::Ok(TransportPrincipal::Anonymous),
        );

        let response = app
            .oneshot(Request::get(format!("/api/v1/shops/{shop_id}")).body(Body::empty())?)
            .await?;

        assert_eq!(StatusCode::NOT_FOUND, response.status());
        assert_eq!(
            json!(SHOP_NOT_FOUND.to_string()),
            body_json(response).await?["error"]
        );
        Ok(())
    }

    fn app(
        shop_result: Result<ShopDetailsView, FakeGetShopError>,
        auth_result: FakeAuthResult,
    ) -> (Router, GetShopCalls) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let state = ShopsState::new(
            Arc::new(FakeGetShopUseCase {
                result: Arc::new(Mutex::new(shop_result)),
                calls: Arc::clone(&calls),
            }),
            Arc::new(UnusedUseCase),
            Arc::new(UnusedUseCase),
            Arc::new(UnusedUseCase),
            Arc::new(UnusedUseCase),
            Arc::new(FakeAuthenticator {
                result: Arc::new(Mutex::new(auth_result)),
            }),
        );
        (
            Router::new()
                .route("/api/v1/shops/{shop_id}", axum::routing::get(get_shop))
                .with_state(state),
            calls,
        )
    }

    struct UnusedUseCase;

    #[async_trait::async_trait]
    impl shop_service::use_cases::queries::search_shops::SearchShopsUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: shop_service::use_cases::queries::search_shops::SearchShopsRequest,
        ) -> Result<
            shop_service::use_cases::queries::search_shops::SearchShopsResult,
            shop_service::use_cases::queries::search_shops::SearchShopsError,
        > {
            unreachable!("unused search use case")
        }
    }

    #[async_trait::async_trait]
    impl shop_service::use_cases::commands::create_shop::CreateShopUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _command: shop_service::use_cases::commands::create_shop::CreateShopCommand,
        ) -> Result<
            shop_service::use_cases::commands::create_shop::CreateShopResult,
            shop_service::use_cases::commands::create_shop::CreateShopError,
        > {
            unreachable!("unused create shop use case")
        }
    }

    #[async_trait::async_trait]
    impl shop_service::use_cases::commands::update_shop::UpdateShopUseCase for UnusedUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _command: shop_service::use_cases::commands::update_shop::UpdateShopCommand,
        ) -> Result<
            shop_service::use_cases::commands::update_shop::UpdateShopResult,
            shop_service::use_cases::commands::update_shop::UpdateShopError,
        > {
            unreachable!("unused update shop use case")
        }
    }

    #[async_trait::async_trait]
    impl shop_service::use_cases::queries::list_user_partner_shops::ListUserPartnerShopsUseCase
        for UnusedUseCase
    {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: shop_service::use_cases::queries::list_user_partner_shops::ListUserPartnerShopsRequest,
        ) -> Result<
            shop_service::use_cases::queries::list_user_partner_shops::ListUserPartnerShopsResult,
            shop_service::use_cases::queries::list_user_partner_shops::ListUserPartnerShopsError,
        > {
            unreachable!("unused partner shops use case")
        }
    }

    async fn body_json(response: Response) -> Result<Value, Box<dyn std::error::Error>> {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn view(shop_id: ShopId) -> ShopDetailsView {
        ShopDetailsView {
            shop_id,
            shop_slug_id: ShopSlugId::from("antik-markt"),
            name: ShopName::from("Antik Markt"),
            shop_type: ShopType::CommercialDealer,
            domains: HashSet::from([Domain::try_from("antik.example")
                .unwrap_or_else(|_| Domain::try_from("example.org").unwrap())]),
            shopify_domain: None,
            shopify_currency: None,
            shopify_language: None,
            woocommerce_currency: Some(Currency::Eur),
            woocommerce_language: Some(Language::De),
            url: Url::parse("https://antik.example/").ok(),
            view_url: Url::parse(
                "https://antik.example/?utm_source=aura_historia&utm_medium=referral",
            )
            .ok(),
            image: None,
            structured_address: None,
            geo_address: None,
            phone: None,
            email: None,
            partner_status: ShopPartnerStatus::Partnered,
            affiliate_configuration: None,
            created: datetime!(1970-01-01 0:00 UTC),
            updated: datetime!(1970-01-01 0:00:02 UTC),
        }
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}
