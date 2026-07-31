use crate::auth::{AuthError, OptionalAuthExtractor, RequestMetadata};
use crate::shops::ShopsState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use common::currency::data::CurrencyData;
use common::language::data::LanguageData;
use common::operation_context::Principal;
use common::shop_id::ShopId;
use geo::data::address_data::{GeoAddressData, StructuredAddressData};
use serde::Serialize;
use serde_email::Email;
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop_type::ShopType;
use shop_service::use_cases::queries::get_shop::{GetShopError, GetShopRequest, ShopDetailsView};
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

const ANONYMOUS_CACHE_CONTROL: &str = "public, max-age=600, s-maxage=3600";
const AUTHENTICATED_CACHE_CONTROL: &str = "no-store";
const CORRELATION_ID_HEADER: &str = "x-correlation-id";

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
        Err(error) => return ApiProblem::from_auth_error(error).into_response(),
    };

    let shop_id = match ShopId::try_from(raw_shop_id.as_str()) {
        Ok(shop_id) => shop_id,
        Err(_) => {
            return ApiProblem::bad_request(
                "INVALID_UUID",
                "Path parameter 'shopId' must be a UUID.",
            )
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
        Ok(view) => shop_response(view, cache_control),
        Err(error) => ApiProblem::from_get_shop_error(error).into_response(),
    }
}

fn shop_response(view: ShopDetailsView, cache_control: &'static str) -> Response {
    let updated = view.updated;
    let mut response = Json(GetShopData::from(view)).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    if let Ok(value) = HeaderValue::from_str(&httpdate::fmt_http_date(updated.into())) {
        headers.insert(header::LAST_MODIFIED, value);
    }
    response
}

fn cache_control(principal: &Principal) -> &'static str {
    match principal {
        Principal::Anonymous => ANONYMOUS_CACHE_CONTROL,
        Principal::User(_)
        | Principal::DelegatedUser { .. }
        | Principal::Service(_)
        | Principal::System => AUTHENTICATED_CACHE_CONTROL,
    }
}

fn request_metadata(headers: &HeaderMap) -> RequestMetadata {
    let request_id = uuid::Uuid::new_v4().to_string();
    let correlation_id = headers
        .get(CORRELATION_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| request_id.clone());
    RequestMetadata::new(request_id, correlation_id)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GetShopData {
    shop_id: ShopId,
    shop_slug_id: common::shop_slug_id::ShopSlugId,
    name: common::shop_name::ShopName,
    shop_type: ShopTypeData,
    domains: HashSet<common::domain::Domain>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shopify_domain: Option<common::domain::Domain>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shopify_currency: Option<CurrencyData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shopify_language: Option<LanguageData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    woocommerce_currency: Option<CurrencyData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    woocommerce_language: Option<LanguageData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    view_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_address: Option<StructuredAddressData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    geo_address: Option<GeoAddressData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<Email>,
    partner_status: ShopPartnerStatusData,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
}

impl From<ShopDetailsView> for GetShopData {
    fn from(view: ShopDetailsView) -> Self {
        Self {
            shop_id: view.shop_id,
            shop_slug_id: view.shop_slug_id,
            name: view.name,
            shop_type: view.shop_type.into(),
            domains: view.domains,
            shopify_domain: view.shopify_domain,
            shopify_currency: view.shopify_currency.map(Into::into),
            shopify_language: view.shopify_language.map(Into::into),
            woocommerce_currency: view.woocommerce_currency.map(Into::into),
            woocommerce_language: view.woocommerce_language.map(Into::into),
            url: view.url,
            view_url: view.view_url,
            image: view.image,
            structured_address: view.structured_address.map(Into::into),
            geo_address: view.geo_address.map(Into::into),
            phone: view.phone,
            email: view.email,
            partner_status: view.partner_status.into(),
            created: view.created,
            updated: view.updated,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ShopTypeData {
    AuctionHouse,
    AuctionPlatform,
    CommercialDealer,
    Marketplace,
}

impl From<ShopType> for ShopTypeData {
    fn from(value: ShopType) -> Self {
        match value {
            ShopType::AuctionHouse => Self::AuctionHouse,
            ShopType::AuctionPlatform => Self::AuctionPlatform,
            ShopType::CommercialDealer => Self::CommercialDealer,
            ShopType::Marketplace => Self::Marketplace,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ShopPartnerStatusData {
    Scraped,
    Partnered,
}

impl From<ShopPartnerStatus> for ShopPartnerStatusData {
    fn from(value: ShopPartnerStatus) -> Self {
        match value {
            ShopPartnerStatus::Scraped => Self::Scraped,
            ShopPartnerStatus::Partnered => Self::Partnered,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProblemData {
    code: &'static str,
    message: &'static str,
}

struct ApiProblem {
    status: StatusCode,
    body: ProblemData,
}

impl ApiProblem {
    fn bad_request(code: &'static str, message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            body: ProblemData { code, message },
        }
    }

    fn from_auth_error(error: AuthError) -> Self {
        match error {
            AuthError::TemporarilyUnavailable => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                body: ProblemData {
                    code: "AUTH_TEMPORARILY_UNAVAILABLE",
                    message: "Authentication is temporarily unavailable.",
                },
            },
            AuthError::Internal(_) => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                body: ProblemData {
                    code: "AUTH_INTERNAL_ERROR",
                    message: "Authentication failed internally.",
                },
            },
            AuthError::MissingCredentials
            | AuthError::InvalidAuthorizationHeader
            | AuthError::MalformedCredentials
            | AuthError::InvalidCredentials
            | AuthError::MissingClaim(_)
            | AuthError::InvalidClaimType(_)
            | AuthError::JwksKeyNotFound
            | AuthError::JwksFetch(_) => Self {
                status: StatusCode::UNAUTHORIZED,
                body: ProblemData {
                    code: "INVALID_CREDENTIALS",
                    message: "Bearer token is invalid.",
                },
            },
        }
    }

    fn from_get_shop_error(error: GetShopError) -> Self {
        match error {
            GetShopError::NotFound => Self {
                status: StatusCode::NOT_FOUND,
                body: ProblemData {
                    code: "SHOP_NOT_FOUND",
                    message: "Shop was not found.",
                },
            },
            GetShopError::TemporarilyUnavailable { .. }
            | GetShopError::BeginTransactionFailed
            | GetShopError::CommitTransactionFailed => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                body: ProblemData {
                    code: "SHOP_TEMPORARILY_UNAVAILABLE",
                    message: "Shop details are temporarily unavailable.",
                },
            },
            GetShopError::InvalidReadModel { .. } | GetShopError::Internal { .. } => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                body: ProblemData {
                    code: "SHOP_INTERNAL_ERROR",
                    message: "Shop details failed internally.",
                },
            },
        }
    }
}

impl IntoResponse for ApiProblem {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthMethod, TokenAuthenticator, TransportPrincipal};
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use common::currency::domain::Currency;
    use common::domain::Domain;
    use common::language::domain::Language;
    use common::operation_context::{CredentialCapability, OperationContext};
    use common::shop_name::ShopName;
    use common::shop_slug_id::ShopSlugId;
    use common::user_id::UserId;
    use serde_json::{Value, json};
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex, MutexGuard};
    use time::macros::datetime;
    use tower::ServiceExt;

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
            ANONYMOUS_CACHE_CONTROL,
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
        assert_eq!(
            AUTHENTICATED_CACHE_CONTROL,
            response.headers()[header::CACHE_CONTROL]
        );
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
        assert_eq!(json!("SHOP_NOT_FOUND"), body_json(response).await?["code"]);
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
