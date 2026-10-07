use crate::error::{
    ApiError, BAD_BODY_VALUE, NEWSLETTER_CONFIRMATION_INVALID, NEWSLETTER_INTERNAL_ERROR,
    NEWSLETTER_TEMPORARILY_UNAVAILABLE,
};
use crate::state::NewsletterState;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde::de::DeserializeOwned;

const MAX_NEWSLETTER_BODY_BYTES: usize = 8 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfirmNewsletterSubscriptionDto {
    token: String,
}

pub async fn confirm_newsletter_subscription(
    State(state): State<NewsletterState>,
    request: Request<Body>,
) -> Response {
    let data: ConfirmNewsletterSubscriptionDto = match parse_body(request.into_body()).await {
        Ok(data) => data,
        Err(error) => return no_store(error.into_response()),
    };

    match state.confirm_subscription.execute(&data.token).await {
        Ok(()) => no_store(StatusCode::NO_CONTENT.into_response()),
        Err(error) => {
            let error = match error {
                user_service::use_cases::ConfirmNewsletterSubscriptionError::InvalidConfirmation => {
                    ApiError::bad_request(NEWSLETTER_CONFIRMATION_INVALID)
                }
                user_service::use_cases::ConfirmNewsletterSubscriptionError::TemporarilyUnavailable => {
                    ApiError::service_unavailable(NEWSLETTER_TEMPORARILY_UNAVAILABLE)
                }
                user_service::use_cases::ConfirmNewsletterSubscriptionError::InvalidPersistedState => {
                    ApiError::internal_server_error(NEWSLETTER_INTERNAL_ERROR)
                }
            };
            no_store(error.into_response())
        }
    }
}

async fn parse_body<T: DeserializeOwned>(body: Body) -> Result<T, ApiError> {
    let bytes = to_bytes(body, MAX_NEWSLETTER_BODY_BYTES)
        .await
        .map_err(|_| ApiError::bad_request(BAD_BODY_VALUE))?;
    serde_json::from_slice(&bytes).map_err(|_| ApiError::bad_request(BAD_BODY_VALUE))
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthError, RequestMetadata, TokenAuthenticator, TransportPrincipal};
    use axum::body::to_bytes;
    use axum::http::{Request as HttpRequest, header};
    use axum::routing::post;
    use std::sync::{Arc, Mutex, MutexGuard};
    use tower::ServiceExt;
    use user_service::use_cases::{
        ConfirmNewsletterSubscriptionError, ConfirmNewsletterSubscriptionUseCase,
    };

    #[derive(Clone, Default)]
    struct RecordingUseCase {
        tokens: Arc<Mutex<Vec<String>>>,
        result: Arc<Mutex<Option<RecordedResult>>>,
    }

    #[derive(Clone, Copy)]
    enum RecordedResult {
        Invalid,
        TemporarilyUnavailable,
        Internal,
    }

    #[async_trait::async_trait]
    impl ConfirmNewsletterSubscriptionUseCase for RecordingUseCase {
        async fn execute(&self, token: &str) -> Result<(), ConfirmNewsletterSubscriptionError> {
            lock(&self.tokens).push(token.to_owned());
            match *lock(&self.result) {
                None => Ok(()),
                Some(RecordedResult::Invalid) => {
                    Err(ConfirmNewsletterSubscriptionError::InvalidConfirmation)
                }
                Some(RecordedResult::TemporarilyUnavailable) => {
                    Err(ConfirmNewsletterSubscriptionError::TemporarilyUnavailable)
                }
                Some(RecordedResult::Internal) => {
                    Err(ConfirmNewsletterSubscriptionError::InvalidPersistedState)
                }
            }
        }
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[derive(Clone, Copy)]
    struct UnusedAuthenticator;

    #[async_trait::async_trait]
    impl TokenAuthenticator for UnusedAuthenticator {
        async fn authenticate(
            &self,
            _bearer_token: &str,
            _metadata: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            unreachable!("confirmation is anonymous and never authenticates")
        }
    }

    #[derive(Clone, Copy)]
    struct RequestNeverCalled;

    #[async_trait::async_trait]
    impl user_service::use_cases::RequestNewsletterSubscriptionUseCase for RequestNeverCalled {
        async fn execute(
            &self,
            _context: &application::operation_context::OperationContext,
            _command: user_service::use_cases::RequestNewsletterSubscriptionCommand,
        ) -> Result<(), user_service::use_cases::RequestNewsletterSubscriptionError> {
            unreachable!("confirmation must not issue a new challenge")
        }
    }

    fn router(use_case: RecordingUseCase) -> axum::Router {
        axum::Router::new()
            .route(
                "/api/v1/newsletter-subscriptions/confirm",
                post(confirm_newsletter_subscription),
            )
            .with_state(NewsletterState::new(
                Arc::new(RequestNeverCalled),
                Arc::new(use_case),
                Arc::new(UnusedAuthenticator),
            ))
    }

    fn request(body: impl Into<Body>) -> HttpRequest<Body> {
        HttpRequest::builder()
            .method("POST")
            .uri("/api/v1/newsletter-subscriptions/confirm")
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.into())
            .unwrap_or_else(|error| panic!("failed to create request: {error}"))
    }

    #[tokio::test]
    async fn anonymous_confirmation_returns_empty_no_store_204_without_bearer() {
        let use_case = RecordingUseCase::default();
        let response = router(use_case.clone())
            .oneshot(request(r#"{"token":"opaque-confirmation-token"}"#))
            .await
            .unwrap_or_else(|error| panic!("failed to call router: {error}"));

        assert_eq!(StatusCode::NO_CONTENT, response.status());
        assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
        assert!(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap_or_else(|error| panic!("failed to read response body: {error}"))
                .is_empty()
        );
        assert_eq!(vec!["opaque-confirmation-token"], *lock(&use_case.tokens));
    }

    #[tokio::test]
    async fn malformed_or_wrong_shape_confirmation_body_has_no_side_effects() {
        let use_case = RecordingUseCase::default();
        for body in [
            "not-json".to_owned(),
            r#"{"token":"secret","extra":true}"#.to_owned(),
            format!(
                "{{\"token\":\"{}\"}}",
                "x".repeat(MAX_NEWSLETTER_BODY_BYTES)
            ),
        ] {
            let response = router(use_case.clone())
                .oneshot(request(body))
                .await
                .unwrap_or_else(|error| panic!("failed to call router: {error}"));
            assert_eq!(StatusCode::BAD_REQUEST, response.status());
            assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
            let bytes = to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap_or_else(|error| panic!("failed to read error body: {error}"));
            let error: serde_json::Value = serde_json::from_slice(&bytes)
                .unwrap_or_else(|error| panic!("failed to parse error body: {error}"));
            assert_eq!("BAD_BODY_VALUE", error["error"]);
            assert!(!error.to_string().contains("secret"));
        }
        assert!(lock(&use_case.tokens).is_empty());
    }

    #[tokio::test]
    async fn invalid_proofs_use_one_generic_error_without_echoing_token() {
        let use_case = RecordingUseCase::default();
        *lock(&use_case.result) = Some(RecordedResult::Invalid);
        let response = router(use_case)
            .oneshot(request(r#"{"token":"private-proof"}"#))
            .await
            .unwrap_or_else(|error| panic!("failed to call router: {error}"));
        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|error| panic!("failed to read error body: {error}"));
        let error: serde_json::Value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|error| panic!("failed to parse error body: {error}"));
        assert_eq!("NEWSLETTER_CONFIRMATION_INVALID", error["error"]);
        assert!(!error.to_string().contains("private-proof"));
    }

    #[tokio::test]
    async fn persistence_and_internal_failures_are_mapped_without_revealing_proof() {
        for (result, status, code) in [
            (
                RecordedResult::TemporarilyUnavailable,
                StatusCode::SERVICE_UNAVAILABLE,
                "NEWSLETTER_TEMPORARILY_UNAVAILABLE",
            ),
            (
                RecordedResult::Internal,
                StatusCode::INTERNAL_SERVER_ERROR,
                "NEWSLETTER_INTERNAL_ERROR",
            ),
        ] {
            let use_case = RecordingUseCase::default();
            *lock(&use_case.result) = Some(result);
            let response = router(use_case)
                .oneshot(request(r#"{"token":"private-proof"}"#))
                .await
                .unwrap_or_else(|error| panic!("failed to call router: {error}"));
            assert_eq!(status, response.status());
            assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
            let bytes = to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap_or_else(|error| panic!("failed to read error body: {error}"));
            let error: serde_json::Value = serde_json::from_slice(&bytes)
                .unwrap_or_else(|error| panic!("failed to parse error body: {error}"));
            assert_eq!(code, error["error"]);
            assert!(!error.to_string().contains("private-proof"));
        }
    }

    #[tokio::test]
    async fn get_confirmation_cannot_mutate_consent() {
        let use_case = RecordingUseCase::default();
        let response = router(use_case.clone())
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/api/v1/newsletter-subscriptions/confirm")
                    .body(Body::empty())
                    .unwrap_or_else(|error| panic!("failed to create request: {error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("failed to call router: {error}"));
        assert_eq!(StatusCode::METHOD_NOT_ALLOWED, response.status());
        assert!(lock(&use_case.tokens).is_empty());
    }
}
