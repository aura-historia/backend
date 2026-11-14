use super::{MAX_CONTENT_NOTICE_BODY_BYTES, types::SubmitContentNoticeData};
use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::ApiError;
use crate::state::ContentNoticesState;
use crate::transport::{cache::no_store, json_body::parse_capped_json};
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use content_notice_service::use_cases::commands::submit_content_notice::{
    SubmitContentNoticeCommand, SubmitContentNoticeError,
};

/// `POST /v1/content-notices`. Anyone may submit; a supplied credential must be valid but is
/// not otherwise used. Every response is `no-store`.
pub(super) async fn submit(
    State(state): State<ContentNoticesState>,
    request: Request<Body>,
) -> Response {
    no_store(match submit_notice(&state, request).await {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(error) => error.into_response(),
    })
}

async fn submit_notice(
    state: &ContentNoticesState,
    request: Request<Body>,
) -> Result<(), ApiError> {
    let headers = request.headers().clone();
    let metadata = request_metadata(&headers);
    let principal = OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
        .map_err(ApiError::from)?;
    let data: SubmitContentNoticeData =
        parse_capped_json(request.into_body(), MAX_CONTENT_NOTICE_BODY_BYTES).await?;
    let command = SubmitContentNoticeCommand::try_from(data)?;
    state
        .submit
        .execute(&principal.operation_context(metadata), command)
        .await
        .map(|_| ())
        .map_err(|error| {
            if let Some(failure) = terminal_failure(&error) {
                tracing::warn!(
                    event = "content_notice.submit_failed",
                    failure,
                    "Content notice submission failed"
                );
            }
            ApiError::from(error)
        })
}

/// The bounded category of a failure that is not the caller's fault, for the terminal log.
fn terminal_failure(error: &SubmitContentNoticeError) -> Option<&'static str> {
    match error {
        SubmitContentNoticeError::InvalidNotice(_) | SubmitContentNoticeError::TargetNotFound => {
            None
        }
        SubmitContentNoticeError::TemporarilyUnavailable { .. } => Some("temporarily_unavailable"),
        SubmitContentNoticeError::InvalidPersistedState { .. } => Some("invalid_persisted_state"),
        SubmitContentNoticeError::Internal { .. } => Some("internal"),
        SubmitContentNoticeError::BeginTransactionFailed(_) => Some("begin_transaction"),
        SubmitContentNoticeError::CommitTransactionFailed(_) => Some("commit_transaction"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use crate::content_notices::router;
    use application::{
        error::static_error,
        operation_context::{OperationContext, Principal},
    };
    use axum::body::to_bytes;
    use axum::http::{Request as HttpRequest, header};
    use content_notice_core::{
        content_notice::ContentNoticeInvariantError, content_notice_id::ContentNoticeId,
        reason_category::ReasonCategory, submitter_role::SubmitterRole,
    };
    use content_notice_service::use_cases::commands::submit_content_notice::{
        ContentNoticeTargetReference, SubmitContentNoticeResult, SubmitContentNoticeUseCase,
    };
    use content_notice_service::use_cases::queries::list_admin_content_notices::{
        ListAdminContentNoticesError, ListAdminContentNoticesRequest,
        ListAdminContentNoticesResult, ListAdminContentNoticesUseCase,
    };
    use listing_source_core::ListingSourceId;
    use product_listing_core::product_listing_id::ProductListingId;
    use serde_json::{Value, json};
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex, MutexGuard};
    use tower::ServiceExt;
    use user_core::user_id::UserId;

    type UseCaseOutcome = fn() -> Result<SubmitContentNoticeResult, SubmitContentNoticeError>;

    #[derive(Clone)]
    struct RecordingUseCase {
        calls: Arc<Mutex<Vec<(Principal, SubmitContentNoticeCommand)>>>,
        outcome: UseCaseOutcome,
    }

    impl RecordingUseCase {
        fn returning(outcome: UseCaseOutcome) -> Self {
            Self {
                calls: Arc::default(),
                outcome,
            }
        }

        fn calls(&self) -> MutexGuard<'_, Vec<(Principal, SubmitContentNoticeCommand)>> {
            self.calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        }
    }

    #[async_trait::async_trait]
    impl SubmitContentNoticeUseCase for RecordingUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            command: SubmitContentNoticeCommand,
        ) -> Result<SubmitContentNoticeResult, SubmitContentNoticeError> {
            self.calls().push((context.principal.clone(), command));
            (self.outcome)()
        }
    }

    fn stored() -> Result<SubmitContentNoticeResult, SubmitContentNoticeError> {
        Ok(SubmitContentNoticeResult {
            content_notice_id: ContentNoticeId::new(),
        })
    }

    /// Accepts the bearer token `valid` as a user session and rejects every other token.
    struct SessionAuthenticator(UserId);

    #[async_trait::async_trait]
    impl TokenAuthenticator for SessionAuthenticator {
        async fn authenticate(
            &self,
            bearer_token: &str,
            _metadata: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            if bearer_token != "valid" {
                return Err(AuthError::InvalidCredentials);
            }
            Ok(TransportPrincipal::User {
                user_id: self.0,
                auth_method: AuthMethod::CognitoJwt,
                capabilities: BTreeSet::new(),
            })
        }
    }

    /// The admin list shares the router; submission tests never reach it.
    struct UnusedListAdmin;

    #[async_trait::async_trait]
    impl ListAdminContentNoticesUseCase for UnusedListAdmin {
        async fn execute(
            &self,
            _: &OperationContext,
            _: ListAdminContentNoticesRequest,
        ) -> Result<ListAdminContentNoticesResult, ListAdminContentNoticesError> {
            unreachable!("submission tests do not list notices")
        }
    }

    fn app(use_case: &RecordingUseCase, user_id: UserId) -> axum::Router {
        router(ContentNoticesState::new(
            Arc::new(use_case.clone()),
            Arc::new(UnusedListAdmin),
            Arc::new(SessionAuthenticator(user_id)),
        ))
    }

    fn body(target: Value) -> Value {
        json!({
            "target": target,
            "submitterRole": "RIGHTS_HOLDER",
            "reasonCategory": "RIGHTS_INFRINGEMENT",
            "explanation": "Das Foto ist meins.",
            "additionalUrls": ["https://portfolio.example/vase"],
            "name": "Erika Mustermann",
            "email": "erika@example.com",
            "language": "de",
            "goodFaithConfirmed": true
        })
    }

    fn listing_source_body(listing_source_id: ListingSourceId) -> Value {
        body(json!({ "type": "LISTING_SOURCE", "listingSourceId": listing_source_id.to_string() }))
    }

    fn with(mut body: Value, field: &str, value: Value) -> Value {
        body[field] = value;
        body
    }

    fn without(mut body: Value, field: &str) -> Value {
        body.as_object_mut().unwrap().remove(field);
        body
    }

    async fn post(app: axum::Router, bearer: Option<&str>, body: impl Into<Body>) -> Response {
        let mut request = HttpRequest::builder()
            .method("POST")
            .uri("/v1/content-notices")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(token) = bearer {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        app.oneshot(request.body(body.into()).unwrap())
            .await
            .unwrap()
    }

    async fn post_json(use_case: &RecordingUseCase, body: &Value) -> Response {
        post(app(use_case, UserId::new()), None, body.to_string()).await
    }

    async fn problem(response: Response) -> (StatusCode, Value) {
        let status = response.status();
        assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn should_accept_both_target_kinds_anonymously_and_with_a_session() {
        let user_id = UserId::new();
        let product_listing_id = ProductListingId::new();
        let listing_source_id = ListingSourceId::new();
        let targets = [
            (
                json!({ "type": "PRODUCT_LISTING", "productListingId": product_listing_id.to_string() }),
                ContentNoticeTargetReference::ProductListing { product_listing_id },
            ),
            (
                json!({ "type": "LISTING_SOURCE", "listingSourceId": listing_source_id.to_string() }),
                ContentNoticeTargetReference::ListingSource { listing_source_id },
            ),
        ];

        for (target, expected_target) in targets {
            for (bearer, expected_principal) in [
                (None, Principal::Anonymous),
                (Some("valid"), Principal::User(user_id)),
            ] {
                let use_case = RecordingUseCase::returning(stored);

                let response = post(
                    app(&use_case, user_id),
                    bearer,
                    body(target.clone()).to_string(),
                )
                .await;

                assert_eq!(StatusCode::ACCEPTED, response.status());
                assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
                assert!(
                    to_bytes(response.into_body(), usize::MAX)
                        .await
                        .unwrap()
                        .is_empty()
                );
                let calls = use_case.calls();
                let (principal, command) = &calls[0];
                assert_eq!(&expected_principal, principal);
                assert_eq!(expected_target, command.target);
                assert_eq!(SubmitterRole::RightsHolder, command.submitter_role);
                assert_eq!(ReasonCategory::RightsInfringement, command.reason_category);
                assert_eq!(
                    vec!["https://portfolio.example/vase"],
                    command
                        .additional_urls
                        .as_slice()
                        .iter()
                        .map(|url| url.as_str())
                        .collect::<Vec<_>>()
                );
                assert_eq!("erika@example.com", command.submitter.email.as_str());
            }
        }
    }

    #[tokio::test]
    async fn should_accept_provider_opt_out_without_explanation_or_urls() {
        let use_case = RecordingUseCase::returning(stored);
        let opt_out = without(
            without(listing_source_body(ListingSourceId::new()), "explanation"),
            "additionalUrls",
        );
        let opt_out = with(
            with(opt_out, "submitterRole", json!("PROVIDER")),
            "reasonCategory",
            json!("PROVIDER_OPT_OUT"),
        );

        let response = post_json(&use_case, &opt_out).await;

        assert_eq!(StatusCode::ACCEPTED, response.status());
        let calls = use_case.calls();
        assert_eq!(None, calls[0].1.explanation);
        assert!(calls[0].1.additional_urls.is_empty());
    }

    #[tokio::test]
    async fn should_reject_invalid_bearer_token_instead_of_treating_caller_as_anonymous() {
        let use_case = RecordingUseCase::returning(stored);

        let response = post(
            app(&use_case, UserId::new()),
            Some("revoked"),
            listing_source_body(ListingSourceId::new()).to_string(),
        )
        .await;

        let (status, problem) = problem(response).await;
        assert_eq!(StatusCode::UNAUTHORIZED, status);
        assert_eq!("INVALID_CREDENTIALS", problem["error"]);
        assert!(use_case.calls().is_empty());
    }

    #[tokio::test]
    async fn should_reject_malformed_unknown_or_oversized_bodies_as_bad_body_value() {
        let valid = listing_source_body(ListingSourceId::new());
        let mut unknown_target_field = valid.clone();
        unknown_target_field["target"]["productListingId"] = json!("pl_x");
        let oversized = with(
            valid.clone(),
            "explanation",
            json!("x".repeat(crate::content_notices::MAX_CONTENT_NOTICE_BODY_BYTES)),
        );

        for body in [
            "not-json".to_owned(),
            without(valid.clone(), "name").to_string(),
            with(valid.clone(), "submitterRole", json!("OPERATOR")).to_string(),
            with(valid.clone(), "ipAddress", json!("192.0.2.1")).to_string(),
            unknown_target_field.to_string(),
            oversized.to_string(),
        ] {
            let use_case = RecordingUseCase::returning(stored);

            let response = post(app(&use_case, UserId::new()), None, body).await;

            let (status, problem) = problem(response).await;
            assert_eq!(StatusCode::BAD_REQUEST, status);
            assert_eq!("BAD_BODY_VALUE", problem["error"]);
            assert!(use_case.calls().is_empty());
        }
    }

    #[tokio::test]
    async fn should_accept_body_at_the_size_cap() {
        let use_case = RecordingUseCase::returning(stored);
        let mut body = listing_source_body(ListingSourceId::new());
        let padding =
            crate::content_notices::MAX_CONTENT_NOTICE_BODY_BYTES - body.to_string().len();
        body["name"] = json!(format!("Erika Mustermann{}", " ".repeat(padding)));
        let encoded = body.to_string();
        assert_eq!(
            crate::content_notices::MAX_CONTENT_NOTICE_BODY_BYTES,
            encoded.len()
        );

        let response = post(app(&use_case, UserId::new()), None, encoded).await;

        assert_eq!(StatusCode::ACCEPTED, response.status());
        assert_eq!(
            "Erika Mustermann",
            use_case.calls()[0].1.submitter.name.as_str()
        );
    }

    #[tokio::test]
    async fn should_point_at_the_rejected_field_without_repeating_its_value() {
        let valid = listing_source_body(ListingSourceId::new());
        let cases = [
            (
                with(
                    valid.clone(),
                    "additionalUrls",
                    json!(["https://a.example/", "ftp://private.example/"]),
                ),
                "CONTENT_NOTICE_INVALID_FIELD",
                "/additionalUrls/1",
            ),
            (
                with(
                    valid.clone(),
                    "email",
                    json!("Private <private@example.com>"),
                ),
                "CONTENT_NOTICE_INVALID_FIELD",
                "/email",
            ),
            (
                with(valid.clone(), "goodFaithConfirmed", json!(false)),
                "CONTENT_NOTICE_GOOD_FAITH_NOT_CONFIRMED",
                "/goodFaithConfirmed",
            ),
            (
                without(valid.clone(), "goodFaithConfirmed"),
                "CONTENT_NOTICE_GOOD_FAITH_NOT_CONFIRMED",
                "/goodFaithConfirmed",
            ),
            (
                with(
                    valid.clone(),
                    "target",
                    json!({ "type": "PRODUCT_LISTING", "productListingId": "ls_not-a-listing" }),
                ),
                "INVALID_OBJECT_ID",
                "/target/productListingId",
            ),
        ];

        for (body, code, pointer) in cases {
            let use_case = RecordingUseCase::returning(stored);

            let (status, problem) = problem(post_json(&use_case, &body).await).await;

            assert_eq!(StatusCode::BAD_REQUEST, status, "{pointer}");
            assert_eq!(code, problem["error"], "{pointer}");
            assert_eq!(
                json!({ "field": pointer, "type": "BODY" }),
                problem["source"]
            );
            assert!(!problem.to_string().contains("private"), "{problem}");
            assert!(use_case.calls().is_empty());
        }
    }

    #[tokio::test]
    async fn should_map_use_case_outcomes_to_problems() {
        let cases: [(UseCaseOutcome, StatusCode, &str, Option<&str>); 6] = [
            (
                || {
                    Err(SubmitContentNoticeError::InvalidNotice(
                        ContentNoticeInvariantError::ExplanationRequired {
                            reason_category: ReasonCategory::Other,
                        },
                    ))
                },
                StatusCode::BAD_REQUEST,
                "CONTENT_NOTICE_EXPLANATION_REQUIRED",
                Some("/explanation"),
            ),
            (
                || {
                    Err(SubmitContentNoticeError::InvalidNotice(
                        ContentNoticeInvariantError::ProviderOptOutRequiresProvider {
                            submitter_role: SubmitterRole::RightsHolder,
                        },
                    ))
                },
                StatusCode::BAD_REQUEST,
                "CONTENT_NOTICE_PROVIDER_OPT_OUT_REQUIRES_PROVIDER",
                Some("/reasonCategory"),
            ),
            (
                || Err(SubmitContentNoticeError::TargetNotFound),
                StatusCode::NOT_FOUND,
                "CONTENT_NOTICE_TARGET_NOT_FOUND",
                None,
            ),
            (
                || {
                    Err(SubmitContentNoticeError::BeginTransactionFailed(
                        static_error("begin"),
                    ))
                },
                StatusCode::SERVICE_UNAVAILABLE,
                "CONTENT_NOTICE_TEMPORARILY_UNAVAILABLE",
                None,
            ),
            (
                || {
                    Err(SubmitContentNoticeError::CommitTransactionFailed(
                        static_error("commit"),
                    ))
                },
                StatusCode::SERVICE_UNAVAILABLE,
                "CONTENT_NOTICE_TEMPORARILY_UNAVAILABLE",
                None,
            ),
            (
                || {
                    Err(SubmitContentNoticeError::InvalidPersistedState {
                        source: static_error("invalid"),
                    })
                },
                StatusCode::INTERNAL_SERVER_ERROR,
                "CONTENT_NOTICE_INTERNAL_ERROR",
                None,
            ),
        ];

        for (outcome, expected_status, code, pointer) in cases {
            let use_case = RecordingUseCase::returning(outcome);

            let (status, problem) =
                problem(post_json(&use_case, &listing_source_body(ListingSourceId::new())).await)
                    .await;

            assert_eq!(expected_status, status, "{code}");
            assert_eq!(code, problem["error"]);
            assert_eq!(pointer, problem["source"]["field"].as_str());
        }
    }
}
