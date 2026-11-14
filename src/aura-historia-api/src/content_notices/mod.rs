mod admin_types;
mod get_admin;
mod list_admin;
mod submit;
#[cfg(test)]
mod test_support;
mod types;

use crate::state::ContentNoticesState;
use axum::{
    Router,
    routing::{get, post},
};

pub(crate) const MAX_CONTENT_NOTICE_BODY_BYTES: usize = 16 * 1024;

pub(crate) fn router(state: ContentNoticesState) -> Router {
    Router::new()
        .route("/v1/content-notices", post(submit::submit))
        .route("/v1/admin/content-notices", get(list_admin::list_admin))
        .route(
            "/v1/admin/content-notices/{content_notice_id}",
            get(get_admin::get_admin),
        )
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use crate::content_notices::test_support::Unused;
    use application::{
        error::static_error,
        operation_context::OperationContext,
        pagination::{Cursor, CursoredResult, ScopeBinding},
    };
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use content_notice_core::{
        content_notice_id::ContentNoticeId,
        content_notice_state::ContentNoticeState,
        content_notice_target::ContentNoticeTarget,
        reason_category::ReasonCategory,
        submitter::{SubmitterEmail, SubmitterName},
        submitter_role::SubmitterRole,
    };
    use content_notice_service::use_cases::queries::list_admin_content_notices::{
        ContentNoticeCursor, ContentNoticeSortKey, ContentNoticeSummary,
        ListAdminContentNoticesError, ListAdminContentNoticesRequest,
        ListAdminContentNoticesResult, ListAdminContentNoticesUseCase,
    };
    use listing_source_core::{ListingSourceId, ListingSourceName};
    use serde_json::{Value, json};
    use std::{collections::BTreeSet, sync::Arc};
    use time::macros::datetime;
    use tower::ServiceExt;
    use user_core::user_id::UserId;

    enum Outcome {
        Page,
        Forbidden,
        CursorScopeMismatch,
        Unavailable,
        Invalid,
    }

    struct FakeListAdmin(Outcome);

    #[async_trait::async_trait]
    impl ListAdminContentNoticesUseCase for FakeListAdmin {
        async fn execute(
            &self,
            _: &OperationContext,
            _: ListAdminContentNoticesRequest,
        ) -> Result<ListAdminContentNoticesResult, ListAdminContentNoticesError> {
            match self.0 {
                Outcome::Page => Ok(CursoredResult {
                    items: vec![ContentNoticeSummary {
                        id: ContentNoticeId::new(),
                        state: ContentNoticeState::Submitted,
                        created: datetime!(2026-11-13 10:44:18.123 UTC),
                        target: ContentNoticeTarget::ListingSource {
                            listing_source_id: ListingSourceId::new(),
                        },
                        listing_source_name: ListingSourceName::try_from("Antik Shop").unwrap(),
                        product_listing_title: None,
                        listing_source_partner: true,
                        submitter_role: SubmitterRole::Provider,
                        reason_category: ReasonCategory::ProviderOptOut,
                        submitter_name: SubmitterName::try_from("Erika Mustermann").unwrap(),
                        submitter_email: SubmitterEmail::try_from("erika@antik.example").unwrap(),
                    }],
                    cursor: Cursor {
                        size: 1,
                        search_after: Some(ContentNoticeCursor {
                            scope: ScopeBinding::builder("test/v1").finish(),
                            sort_key: ContentNoticeSortKey::Created(datetime!(
                                2026-11-13 10:44:18.123 UTC
                            )),
                            id: ContentNoticeId::new(),
                        }),
                    },
                    total: Some(7),
                }),
                Outcome::Forbidden => Err(ListAdminContentNoticesError::Forbidden),
                Outcome::CursorScopeMismatch => {
                    Err(ListAdminContentNoticesError::CursorScopeMismatch)
                }
                Outcome::Unavailable => Err(ListAdminContentNoticesError::CommitTransactionFailed(
                    static_error("commit failed"),
                )),
                Outcome::Invalid => Err(ListAdminContentNoticesError::InvalidReadModel {
                    source: static_error("invalid row"),
                }),
            }
        }
    }

    struct FakeAuthenticator(bool);

    #[async_trait::async_trait]
    impl TokenAuthenticator for FakeAuthenticator {
        async fn authenticate(
            &self,
            _: &str,
            _: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            if !self.0 {
                return Err(AuthError::InvalidCredentials);
            }
            Ok(TransportPrincipal::User {
                user_id: UserId::new(),
                auth_method: AuthMethod::CognitoJwt,
                capabilities: BTreeSet::new(),
            })
        }
    }

    async fn get(
        outcome: Outcome,
        authenticated: bool,
        query: &str,
    ) -> (StatusCode, Option<String>, Value) {
        let router = router(ContentNoticesState::new(
            Arc::new(Unused),
            Arc::new(FakeListAdmin(outcome)),
            Arc::new(Unused),
            Arc::new(FakeAuthenticator(authenticated)),
        ));
        let request = Request::builder()
            .uri(format!("/v1/admin/content-notices{query}"))
            .header("Authorization", "Bearer test")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(request).await.unwrap();
        let status = response.status();
        let cache_control = response
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (
            status,
            cache_control,
            serde_json::from_slice(&body).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn should_return_a_cursored_page_without_total_and_never_store_it() {
        let (status, cache_control, body) = get(Outcome::Page, true, "").await;

        assert_eq!(StatusCode::OK, status);
        assert_eq!(Some("no-store".to_owned()), cache_control);
        assert_eq!(json!(1), body["size"]);
        assert!(body["searchAfter"].is_string());
        assert!(body.get("total").is_none());
        assert_eq!(json!("LISTING_SOURCE"), body["items"][0]["target"]["type"]);
        assert_eq!(
            json!("erika@antik.example"),
            body["items"][0]["submitterEmail"]
        );
    }

    #[tokio::test]
    async fn should_map_errors_and_never_store_them() {
        for (outcome, authenticated, query, expected_status, expected_error) in [
            (
                Outcome::Page,
                false,
                "",
                StatusCode::UNAUTHORIZED,
                "INVALID_CREDENTIALS",
            ),
            (
                Outcome::Page,
                true,
                "?sort=created",
                StatusCode::BAD_REQUEST,
                "BAD_ORDER_VALUE",
            ),
            (
                Outcome::Forbidden,
                true,
                "",
                StatusCode::FORBIDDEN,
                "FORBIDDEN",
            ),
            (
                Outcome::CursorScopeMismatch,
                true,
                "",
                StatusCode::BAD_REQUEST,
                "BAD_QUERY_PARAMETER_VALUE",
            ),
            (
                Outcome::Unavailable,
                true,
                "",
                StatusCode::SERVICE_UNAVAILABLE,
                "CONTENT_NOTICE_TEMPORARILY_UNAVAILABLE",
            ),
            (
                Outcome::Invalid,
                true,
                "",
                StatusCode::INTERNAL_SERVER_ERROR,
                "CONTENT_NOTICE_INTERNAL_ERROR",
            ),
        ] {
            let (status, cache_control, body) = get(outcome, authenticated, query).await;

            assert_eq!(expected_status, status, "{expected_error}");
            assert_eq!(
                Some("no-store".to_owned()),
                cache_control,
                "{expected_error}"
            );
            assert_eq!(json!(expected_error), body["error"]);
        }
    }
}
