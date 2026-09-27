use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{ApiError, BAD_BODY_VALUE};
use crate::state::NewsletterState;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use localization::Language;
use money::Currency;
use serde::Deserialize;
use serde_email::Email;
use user_core::{first_name::FirstName, last_name::LastName};
use user_service::use_cases::commands::upsert_newsletter_subscription::UpsertNewsletterSubscriptionCommand;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PutNewsletterSubscriptionDto {
    email: Email,
    #[serde(default)]
    first_name: Option<FirstName>,
    #[serde(default)]
    last_name: Option<LastName>,
    #[serde(default)]
    #[serde(with = "crate::wire::language::option")]
    language: Option<Language>,
    #[serde(default, with = "crate::wire::currency::option")]
    currency: Option<Currency>,
}

impl From<PutNewsletterSubscriptionDto> for UpsertNewsletterSubscriptionCommand {
    fn from(dto: PutNewsletterSubscriptionDto) -> Self {
        Self {
            email: dto.email,
            first_name: dto.first_name,
            last_name: dto.last_name,
            language: dto.language,
            currency: dto.currency,
        }
    }
}

pub async fn put_newsletter_subscription(
    State(state): State<NewsletterState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let data = match parse_body(&body) {
        Ok(data) => data,
        Err(error) => return error.into_response(),
    };
    match state
        .upsert_subscription
        .execute(&principal.operation_context(metadata), data.into())
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn parse_body(body: &str) -> Result<PutNewsletterSubscriptionDto, ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail("Body cannot be empty"));
    }
    serde_json::from_str(body)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthError, RequestMetadata, TokenAuthenticator, TransportPrincipal};
    use application::operation_context::OperationContext;
    use axum::body::Body;
    use axum::http::{Request, header};
    use axum::routing::put;
    use std::sync::{Arc, Mutex, MutexGuard};
    use tower::ServiceExt;
    use user_loops::{LoopsNewsletterConfig, LoopsNewsletterSubscriptionWriter};
    use user_service::ports::{NewsletterProfileReadError, NewsletterProfileReader};
    use user_service::use_cases::commands::upsert_newsletter_subscription::{
        UpsertNewsletterSubscriptionError, UpsertNewsletterSubscriptionHandler,
        UpsertNewsletterSubscriptionUseCase,
    };
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, method, path},
    };

    #[derive(Clone)]
    enum AuthenticationResult {
        Principal(TransportPrincipal),
        InvalidCredentials,
    }

    #[derive(Clone)]
    struct StaticAuthenticator {
        result: AuthenticationResult,
    }

    #[async_trait::async_trait]
    impl TokenAuthenticator for StaticAuthenticator {
        async fn authenticate(
            &self,
            _bearer_token: &str,
            _metadata: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            match &self.result {
                AuthenticationResult::Principal(principal) => Ok(principal.clone()),
                AuthenticationResult::InvalidCredentials => Err(AuthError::InvalidCredentials),
            }
        }
    }

    #[derive(Clone, Copy)]
    struct EmptyNewsletterProfileReader;

    #[async_trait::async_trait]
    impl NewsletterProfileReader for EmptyNewsletterProfileReader {
        async fn find_by_user_id(
            &self,
            _user_id: user_core::user_id::UserId,
        ) -> Result<Option<user_service::ports::NewsletterProfile>, NewsletterProfileReadError>
        {
            Ok(None)
        }
    }

    #[derive(Clone, Default)]
    struct RecordingUseCase {
        commands: Arc<Mutex<Vec<UpsertNewsletterSubscriptionCommand>>>,
        result: Arc<Mutex<Option<UseCaseResult>>>,
    }

    #[derive(Clone, Copy)]
    enum UseCaseResult {
        InvalidEmail,
        TemporarilyUnavailable,
        Internal,
    }

    #[async_trait::async_trait]
    impl UpsertNewsletterSubscriptionUseCase for RecordingUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            command: UpsertNewsletterSubscriptionCommand,
        ) -> Result<(), UpsertNewsletterSubscriptionError> {
            lock(&self.commands).push(command);
            match *lock(&self.result) {
                None => Ok(()),
                Some(UseCaseResult::InvalidEmail) => {
                    Err(UpsertNewsletterSubscriptionError::InvalidEmail)
                }
                Some(UseCaseResult::TemporarilyUnavailable) => Err(
                    UpsertNewsletterSubscriptionError::NewsletterSubscriptionUnavailable {
                        source: application::error::static_error("unavailable"),
                    },
                ),
                Some(UseCaseResult::Internal) => Err(
                    UpsertNewsletterSubscriptionError::NewsletterSubscriptionInternal {
                        source: application::error::static_error("internal"),
                    },
                ),
            }
        }
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn router(authenticator: StaticAuthenticator, use_case: RecordingUseCase) -> axum::Router {
        axum::Router::new()
            .route(
                "/api/v1/newsletter-subscriptions",
                put(put_newsletter_subscription),
            )
            .with_state(NewsletterState::new(
                Arc::new(use_case),
                Arc::new(authenticator),
            ))
    }

    fn loops_router(server: &MockServer, auth_result: AuthenticationResult) -> axum::Router {
        let config = LoopsNewsletterConfig::new(
            "test-loops-api-key".into(),
            "test-newsletter-list".into(),
            format!("{}/api", server.uri()),
        )
        .unwrap_or_else(|error| panic!("invalid test Loops config: {error}"));
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_else(|error| panic!("failed to build test Loops client: {error}"));
        let writer = LoopsNewsletterSubscriptionWriter::new(config, client);
        let handler =
            UpsertNewsletterSubscriptionHandler::new(EmptyNewsletterProfileReader, writer);

        axum::Router::new()
            .route(
                "/api/v1/newsletter-subscriptions",
                put(put_newsletter_subscription),
            )
            .with_state(NewsletterState::new(
                Arc::new(handler),
                Arc::new(StaticAuthenticator {
                    result: auth_result,
                }),
            ))
    }

    fn request(body: impl Into<String>) -> Request<Body> {
        Request::builder()
            .method("PUT")
            .uri("/api/v1/newsletter-subscriptions")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.into()))
            .unwrap_or_else(|error| panic!("failed to create request: {error}"))
    }

    #[tokio::test]
    async fn should_return_204_and_map_request_for_anonymous_subscription() {
        let use_case = RecordingUseCase::default();
        let response = router(
            StaticAuthenticator {
                result: AuthenticationResult::Principal(TransportPrincipal::Anonymous),
            },
            use_case.clone(),
        )
        .oneshot(request(
            r#"{"email":"ada@example.com","language":"en","currency":"EUR"}"#,
        ))
        .await
        .unwrap_or_else(|error| panic!("failed to call router: {error}"));

        assert_eq!(StatusCode::NO_CONTENT, response.status());
        let commands = lock(&use_case.commands);
        assert_eq!(1, commands.len());
        assert_eq!("ada@example.com", commands[0].email.to_string());
        assert_eq!(Some(localization::Language::En), commands[0].language);
        assert_eq!(Some(money::Currency::Eur), commands[0].currency);
    }

    #[tokio::test]
    async fn should_allow_missing_bearer_token_for_public_subscription() {
        let response = router(
            StaticAuthenticator {
                result: AuthenticationResult::Principal(TransportPrincipal::Anonymous),
            },
            RecordingUseCase::default(),
        )
        .oneshot(request(r#"{"email":"ada@example.com"}"#))
        .await
        .unwrap_or_else(|error| panic!("failed to call router: {error}"));

        assert_eq!(StatusCode::NO_CONTENT, response.status());
    }

    #[tokio::test]
    async fn should_reject_invalid_supplied_bearer_token() {
        let response = router(
            StaticAuthenticator {
                result: AuthenticationResult::InvalidCredentials,
            },
            RecordingUseCase::default(),
        )
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/newsletter-subscriptions")
                .header(header::AUTHORIZATION, "Bearer invalid")
                .body(Body::from(r#"{"email":"ada@example.com"}"#))
                .unwrap_or_else(|error| panic!("failed to create request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("failed to call router: {error}"));

        assert_eq!(StatusCode::UNAUTHORIZED, response.status());
    }

    #[tokio::test]
    async fn should_return_bad_body_value_for_empty_or_invalid_body() {
        for body in ["", "not-json", r#"{"email":"not-an-email"}"#] {
            let response = router(
                StaticAuthenticator {
                    result: AuthenticationResult::Principal(TransportPrincipal::Anonymous),
                },
                RecordingUseCase::default(),
            )
            .oneshot(request(body))
            .await
            .unwrap_or_else(|error| panic!("failed to call router: {error}"));

            assert_eq!(StatusCode::BAD_REQUEST, response.status());
        }
    }

    #[tokio::test]
    async fn should_map_subscription_errors_to_stable_http_statuses() {
        for (result, status, error_code) in [
            (
                UseCaseResult::InvalidEmail,
                StatusCode::BAD_REQUEST,
                "INVALID_EMAIL",
            ),
            (
                UseCaseResult::TemporarilyUnavailable,
                StatusCode::SERVICE_UNAVAILABLE,
                "NEWSLETTER_TEMPORARILY_UNAVAILABLE",
            ),
            (
                UseCaseResult::Internal,
                StatusCode::INTERNAL_SERVER_ERROR,
                "NEWSLETTER_INTERNAL_ERROR",
            ),
        ] {
            let use_case = RecordingUseCase::default();
            *lock(&use_case.result) = Some(result);
            let response = router(
                StaticAuthenticator {
                    result: AuthenticationResult::Principal(TransportPrincipal::Anonymous),
                },
                use_case,
            )
            .oneshot(request(r#"{"email":"ada@example.com"}"#))
            .await
            .unwrap_or_else(|error| panic!("failed to call router: {error}"));

            assert_eq!(status, response.status());
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap_or_else(|error| panic!("failed to read error response: {error}"));
            let problem: serde_json::Value = serde_json::from_slice(&body)
                .unwrap_or_else(|error| panic!("invalid problem response: {error}"));
            assert_eq!(error_code, problem["error"]);
        }
    }

    #[tokio::test]
    async fn should_compose_real_loops_writer_through_http_endpoint_and_return_empty_204() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/contacts/update"))
            .and(body_json(serde_json::json!({
                "email": "ada@example.com",
                "source": "aura-historia-newsletter-api",
                "mailingLists": {"test-newsletter-list": true}
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "success": true,
                "id": "test-contact-id"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let app = loops_router(
            &server,
            AuthenticationResult::Principal(TransportPrincipal::Anonymous),
        );

        let response = app
            .oneshot(request(r#"{"email":"ada@example.com"}"#))
            .await
            .unwrap_or_else(|error| panic!("failed to call composed router: {error}"));
        assert_eq!(StatusCode::NO_CONTENT, response.status());
        let response_body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|error| panic!("failed to read response: {error}"));
        assert!(response_body.is_empty());
        assert_eq!(
            1,
            server
                .received_requests()
                .await
                .unwrap_or_else(|| panic!("request recording unavailable"))
                .len()
        );
    }

    #[tokio::test]
    async fn should_map_loops_service_unavailable_to_public_503() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/contacts/update"))
            .respond_with(ResponseTemplate::new(503))
            .expect(1)
            .mount(&server)
            .await;
        let app = loops_router(
            &server,
            AuthenticationResult::Principal(TransportPrincipal::Anonymous),
        );

        let response = app
            .oneshot(request(r#"{"email":"ada@example.com"}"#))
            .await
            .unwrap_or_else(|error| panic!("failed to call composed router: {error}"));
        assert_eq!(StatusCode::SERVICE_UNAVAILABLE, response.status());
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|error| panic!("failed to read error response: {error}"));
        let problem: serde_json::Value = serde_json::from_slice(&body)
            .unwrap_or_else(|error| panic!("invalid problem response: {error}"));
        assert_eq!("NEWSLETTER_TEMPORARILY_UNAVAILABLE", problem["error"]);
    }

    #[tokio::test]
    async fn should_not_call_loops_for_invalid_authentication_or_request_body() {
        let server = MockServer::start().await;
        let app = loops_router(&server, AuthenticationResult::InvalidCredentials);

        let invalid_auth = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/v1/newsletter-subscriptions")
                    .header(header::AUTHORIZATION, "Bearer invalid")
                    .body(Body::from(r#"{"email":"ada@example.com"}"#))
                    .unwrap_or_else(|error| panic!("failed to create request: {error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("failed to call router: {error}"));
        assert_eq!(StatusCode::UNAUTHORIZED, invalid_auth.status());

        let invalid_body = app
            .oneshot(request("not-json"))
            .await
            .unwrap_or_else(|error| panic!("failed to call router: {error}"));
        assert_eq!(StatusCode::BAD_REQUEST, invalid_body.status());

        assert!(
            server
                .received_requests()
                .await
                .unwrap_or_else(|| panic!("request recording unavailable"))
                .is_empty()
        );
    }
}
