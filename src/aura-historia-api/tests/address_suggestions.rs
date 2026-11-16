use application::operation_context::CredentialCapability;
use aura_historia_api::{
    app,
    auth::{AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal},
    state::{AddressSuggestionsState, AppState},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use geo_postgres::SqlxSuggestionUsageRepositoryFactory;
use geo_service::{
    geocoding::{GeocodingError, GeocodingFailure, PlaceReference},
    suggestions::{
        AddressSuggestion, AddressSuggestionProvider, AddressSuggestions, MAX_PER_DAY,
        SuggestAddressesHandler, SuggestionAttribution, SuggestionKind, SuggestionQuery,
    },
};
use localization::Language;
use platform_postgres::SqlxUnitOfWork;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use tower::ServiceExt;
use user_core::user_id::UserId;

const SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");

struct Auth(UserId);
#[async_trait::async_trait]
impl TokenAuthenticator for Auth {
    async fn authenticate(
        &self,
        token: &str,
        _: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError> {
        let auth_method = match token {
            "person" => AuthMethod::CognitoJwt,
            "agent" => AuthMethod::AuraAccessToken,
            _ => return Err(AuthError::InvalidCredentials),
        };
        Ok(TransportPrincipal::User {
            user_id: self.0,
            auth_method,
            capabilities: [CredentialCapability::UsersWrite].into(),
        })
    }
}

/// Counts calls and answers `outage` queries with a provider outage.
struct Provider(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl AddressSuggestionProvider for Provider {
    async fn suggest(
        &self,
        query: &SuggestionQuery,
        language: Language,
    ) -> Result<AddressSuggestions, GeocodingError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        if query.as_str() == "outage" {
            return Err(GeocodingError {
                kind: GeocodingFailure::Unavailable,
                source: Box::new(std::io::Error::other("synthetic outage")),
            });
        }
        Ok(AddressSuggestions {
            suggestions: vec![AddressSuggestion {
                place: PlaceReference::new("synthetic-place").unwrap(),
                label: format!("{} ({})", query.as_str(), language.as_str()),
                kind: SuggestionKind::Address,
            }],
            attribution: SuggestionAttribution {
                provider: "TEST",
                attribution_url: "https://example.test/attribution",
            },
        })
    }
}

async fn post(router: &axum::Router, token: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut r = Request::builder()
        .method("POST")
        .uri("/v1/address-suggestions")
        .header("content-type", "application/json");
    if let Some(token) = token {
        r = r.header("authorization", format!("Bearer {token}"));
    }
    let response = router
        .clone()
        .oneshot(r.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    assert_eq!("no-store", response.headers().get("cache-control").unwrap());
    let status = response.status();
    if status == StatusCode::TOO_MANY_REQUESTS {
        assert!(response.headers().contains_key("retry-after"));
    }
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn should_suggest_addresses_to_signed_in_people_within_their_rate_limit() {
    let pool = get_postgres_client().await;
    let user = UserId::new();
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
        .bind(user.into_uuid())
        .bind(format!("{user}@example.test"))
        .execute(&pool)
        .await
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let router = app(
        AppState::new().with_address_suggestions(AddressSuggestionsState {
            suggest: Arc::new(SuggestAddressesHandler::new(
                SqlxUnitOfWork::new(pool.clone()),
                SqlxSuggestionUsageRepositoryFactory,
                Provider(Arc::clone(&calls)),
            )),
            authenticator: Arc::new(Auth(user)),
        }),
    );
    let query = json!({"query": "Synthetic Street 1", "language": "de"});

    assert_eq!(
        StatusCode::UNAUTHORIZED,
        post(&router, None, query.clone()).await.0
    );
    assert_eq!(
        StatusCode::FORBIDDEN,
        post(&router, Some("agent"), query.clone()).await.0
    );
    let with_limit = json!({"query": "Synthetic Street 1", "language": "de", "limit": 10});
    assert_eq!(
        StatusCode::BAD_REQUEST,
        post(&router, Some("person"), with_limit).await.0
    );
    assert_eq!(0, calls.load(Ordering::SeqCst));

    let (status, body) = post(&router, Some("person"), query.clone()).await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!(
        json!({
            "suggestions": [{"placeId": "synthetic-place", "label": "Synthetic Street 1 (de)", "kind": "ADDRESS"}],
            "attribution": {"provider": "TEST", "attributionUrl": "https://example.test/attribution"}
        }),
        body
    );
    let (status, body) = post(
        &router,
        Some("person"),
        json!({"query": "outage", "language": "en"}),
    )
    .await;
    assert_eq!(StatusCode::SERVICE_UNAVAILABLE, status);
    assert_eq!(json!("ADDRESS_SUGGESTIONS_UNAVAILABLE"), body["error"]);

    // Spending the day's budget refuses further requests without calling the provider.
    sqlx::query("UPDATE address_suggestion_usage SET day_uses=$2 WHERE user_id=$1")
        .bind(user.into_uuid())
        .bind(MAX_PER_DAY as i32)
        .execute(&pool)
        .await
        .unwrap();
    let spent = calls.load(Ordering::SeqCst);
    let (status, body) = post(&router, Some("person"), query).await;
    assert_eq!(StatusCode::TOO_MANY_REQUESTS, status);
    assert_eq!(json!("ADDRESS_SUGGESTIONS_RATE_LIMITED"), body["error"]);
    assert_eq!(spent, calls.load(Ordering::SeqCst));
}
