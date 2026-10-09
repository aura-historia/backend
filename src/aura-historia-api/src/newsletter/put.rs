use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{ApiError, NEWSLETTER_INTERNAL_ERROR, NEWSLETTER_TEMPORARILY_UNAVAILABLE};
use crate::newsletter::common::{no_store, parse_body};
use crate::state::NewsletterState;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use localization::Language;
use money::Currency;
use serde::Deserialize;
use serde_email::Email;
use user_core::{first_name::FirstName, last_name::LastName};
use user_service::use_cases::RequestNewsletterSubscriptionCommand;

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

impl From<PutNewsletterSubscriptionDto> for RequestNewsletterSubscriptionCommand {
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
    request: Request<Body>,
) -> Response {
    let headers = request.headers().clone();
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return no_store(ApiError::from(error).into_response()),
    };
    let data: PutNewsletterSubscriptionDto = match parse_body(request.into_body()).await {
        Ok(data) => data,
        Err(error) => return no_store(error.into_response()),
    };

    match state
        .request_subscription
        .execute(&principal.operation_context(metadata), data.into())
        .await
    {
        Ok(()) => no_store(StatusCode::NO_CONTENT.into_response()),
        Err(error) => {
            let error = match error {
                user_service::use_cases::RequestNewsletterSubscriptionError::TransactionFailed(_)
                | user_service::use_cases::RequestNewsletterSubscriptionError::TemporarilyUnavailable
                | user_service::use_cases::RequestNewsletterSubscriptionError::EmailTemporarilyRejected
                | user_service::use_cases::RequestNewsletterSubscriptionError::EmailAcceptanceUnknown => {
                    ApiError::service_unavailable(NEWSLETTER_TEMPORARILY_UNAVAILABLE)
                }
                user_service::use_cases::RequestNewsletterSubscriptionError::InvalidPersistedState
                | user_service::use_cases::RequestNewsletterSubscriptionError::EmailRejected
                | user_service::use_cases::RequestNewsletterSubscriptionError::TokenGenerationFailed => {
                    ApiError::internal_server_error(NEWSLETTER_INTERNAL_ERROR)
                }
            };
            no_store(error.into_response())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use crate::newsletter::common::MAX_NEWSLETTER_BODY_BYTES;
    use application::operation_context::{OperationContext, Principal};
    use axum::body::to_bytes;
    use axum::http::{Request as HttpRequest, header};
    use axum::routing::put;
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex, MutexGuard};
    use tower::ServiceExt;
    use user_core::user_id::UserId;
    use user_service::use_cases::{
        RequestNewsletterSubscriptionError, RequestNewsletterSubscriptionUseCase,
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

    #[derive(Clone, Default)]
    struct RecordingUseCase {
        contexts: Arc<Mutex<Vec<Principal>>>,
        emails: Arc<Mutex<Vec<String>>>,
        result: Arc<Mutex<Option<UseCaseResult>>>,
    }

    #[derive(Clone, Copy)]
    enum UseCaseResult {
        TemporarilyUnavailable,
        EmailAcceptanceUnknown,
        EmailTemporarilyRejected,
        EmailRejected,
        Internal,
    }

    #[async_trait::async_trait]
    impl RequestNewsletterSubscriptionUseCase for RecordingUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            command: RequestNewsletterSubscriptionCommand,
        ) -> Result<(), RequestNewsletterSubscriptionError> {
            lock(&self.contexts).push(context.principal.clone());
            lock(&self.emails).push(command.email.to_string());
            match *lock(&self.result) {
                None => Ok(()),
                Some(UseCaseResult::TemporarilyUnavailable) => {
                    Err(RequestNewsletterSubscriptionError::TemporarilyUnavailable)
                }
                Some(UseCaseResult::EmailAcceptanceUnknown) => {
                    Err(RequestNewsletterSubscriptionError::EmailAcceptanceUnknown)
                }
                Some(UseCaseResult::EmailTemporarilyRejected) => {
                    Err(RequestNewsletterSubscriptionError::EmailTemporarilyRejected)
                }
                Some(UseCaseResult::EmailRejected) => {
                    Err(RequestNewsletterSubscriptionError::EmailRejected)
                }
                Some(UseCaseResult::Internal) => {
                    Err(RequestNewsletterSubscriptionError::InvalidPersistedState)
                }
            }
        }
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn user_principal(id: UserId) -> TransportPrincipal {
        TransportPrincipal::User {
            user_id: id,
            auth_method: AuthMethod::CognitoJwt,
            capabilities: BTreeSet::new(),
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
                Arc::new(ConfirmNeverCalled),
                Arc::new(authenticator),
            ))
    }

    #[derive(Clone, Copy)]
    struct ConfirmNeverCalled;

    #[async_trait::async_trait]
    impl user_service::use_cases::ConfirmNewsletterSubscriptionUseCase for ConfirmNeverCalled {
        async fn execute(
            &self,
            _token: &str,
        ) -> Result<(), user_service::use_cases::ConfirmNewsletterSubscriptionError> {
            unreachable!("PUT must not invoke confirmation")
        }
    }

    fn request(body: impl Into<Body>) -> HttpRequest<Body> {
        HttpRequest::builder()
            .method("PUT")
            .uri("/api/v1/newsletter-subscriptions")
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.into())
            .unwrap_or_else(|error| panic!("failed to create request: {error}"))
    }

    fn anonymous_authenticator() -> StaticAuthenticator {
        StaticAuthenticator {
            result: AuthenticationResult::Principal(TransportPrincipal::Anonymous),
        }
    }

    async fn empty_body(response: Response) -> bool {
        to_bytes(response.into_body(), usize::MAX)
            .await
            .is_ok_and(|body| body.is_empty())
    }

    #[tokio::test]
    async fn anonymous_put_requests_confirmation_and_returns_empty_no_store_204() {
        let use_case = RecordingUseCase::default();
        let response = router(anonymous_authenticator(), use_case.clone())
            .oneshot(request(
                r#"{"email":"ada@example.com","language":"en","currency":"EUR"}"#,
            ))
            .await
            .unwrap_or_else(|error| panic!("failed to call router: {error}"));

        assert_eq!(StatusCode::NO_CONTENT, response.status());
        assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
        assert!(empty_body(response).await);
        assert_eq!(vec!["ada@example.com"], *lock(&use_case.emails));
        assert_eq!(vec![Principal::Anonymous], *lock(&use_case.contexts));
    }

    #[tokio::test]
    async fn authenticated_own_and_alternate_email_requests_both_reach_doi_use_case() {
        let user_id = UserId::new();
        let account_email = format!("{}@example.test", user_id.as_uuid());
        for email in [account_email.as_str(), "alternate@example.com"] {
            let use_case = RecordingUseCase::default();
            let response = router(
                StaticAuthenticator {
                    result: AuthenticationResult::Principal(user_principal(user_id)),
                },
                use_case.clone(),
            )
            .oneshot(
                HttpRequest::builder()
                    .method("PUT")
                    .uri("/api/v1/newsletter-subscriptions")
                    .header(header::AUTHORIZATION, "Bearer valid")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(r#"{{"email":"{email}"}}"#)))
                    .unwrap_or_else(|error| panic!("failed to create request: {error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("failed to call router: {error}"));

            assert_eq!(StatusCode::NO_CONTENT, response.status());
            assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
            assert!(empty_body(response).await);
            assert_eq!(vec![email], *lock(&use_case.emails));
            assert_eq!(vec![Principal::User(user_id)], *lock(&use_case.contexts));
        }
    }

    #[tokio::test]
    async fn invalid_bearer_and_invalid_or_oversized_bodies_have_no_use_case_side_effects() {
        let use_case = RecordingUseCase::default();
        let app = router(
            StaticAuthenticator {
                result: AuthenticationResult::InvalidCredentials,
            },
            use_case.clone(),
        );
        let invalid_auth = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("PUT")
                    .uri("/api/v1/newsletter-subscriptions")
                    .header(header::AUTHORIZATION, "Bearer invalid")
                    .body(Body::from(r#"{"email":"ada@example.com"}"#))
                    .unwrap_or_else(|error| panic!("failed to create request: {error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("failed to call router: {error}"));
        assert_eq!(StatusCode::UNAUTHORIZED, invalid_auth.status());
        assert_eq!("no-store", invalid_auth.headers()[header::CACHE_CONTROL]);

        let invalid_body = router(anonymous_authenticator(), use_case.clone())
            .oneshot(request("not-json"))
            .await
            .unwrap_or_else(|error| panic!("failed to call router: {error}"));
        assert_eq!(StatusCode::BAD_REQUEST, invalid_body.status());
        assert_eq!("no-store", invalid_body.headers()[header::CACHE_CONTROL]);

        let oversized = router(anonymous_authenticator(), use_case.clone())
            .oneshot(request(Body::from(vec![
                b'a';
                MAX_NEWSLETTER_BODY_BYTES + 1
            ])))
            .await
            .unwrap_or_else(|error| panic!("failed to call router: {error}"));
        assert_eq!(StatusCode::BAD_REQUEST, oversized.status());
        assert_eq!("no-store", oversized.headers()[header::CACHE_CONTROL]);
        assert!(lock(&use_case.emails).is_empty());
    }

    #[tokio::test]
    async fn failures_map_to_safe_codes_and_never_include_body_values() {
        for (result, status, expected_error) in [
            (
                UseCaseResult::TemporarilyUnavailable,
                StatusCode::SERVICE_UNAVAILABLE,
                "NEWSLETTER_TEMPORARILY_UNAVAILABLE",
            ),
            (
                UseCaseResult::EmailAcceptanceUnknown,
                StatusCode::SERVICE_UNAVAILABLE,
                "NEWSLETTER_TEMPORARILY_UNAVAILABLE",
            ),
            (
                UseCaseResult::EmailTemporarilyRejected,
                StatusCode::SERVICE_UNAVAILABLE,
                "NEWSLETTER_TEMPORARILY_UNAVAILABLE",
            ),
            (
                UseCaseResult::Internal,
                StatusCode::INTERNAL_SERVER_ERROR,
                "NEWSLETTER_INTERNAL_ERROR",
            ),
            (
                UseCaseResult::EmailRejected,
                StatusCode::INTERNAL_SERVER_ERROR,
                "NEWSLETTER_INTERNAL_ERROR",
            ),
        ] {
            let use_case = RecordingUseCase::default();
            *lock(&use_case.result) = Some(result);
            let response = router(anonymous_authenticator(), use_case)
                .oneshot(request(
                    r#"{"email":"private@example.com","firstName":"Private"}"#,
                ))
                .await
                .unwrap_or_else(|error| panic!("failed to call router: {error}"));
            assert_eq!(status, response.status());
            assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
            let bytes = to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap_or_else(|error| panic!("failed to read error body: {error}"));
            let body: serde_json::Value = serde_json::from_slice(&bytes)
                .unwrap_or_else(|error| panic!("failed to parse error body: {error}"));
            assert_eq!(expected_error, body["error"]);
            assert!(!body.to_string().contains("private@example.com"));
            assert!(!body.to_string().contains("Private"));
        }
    }
}
