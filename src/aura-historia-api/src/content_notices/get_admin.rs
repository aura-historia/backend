use super::admin_types::AdminContentNoticeDetailsData;
use crate::{
    auth::protected_context, error::ApiError, state::ContentNoticesState,
    transport::cache::no_store, wire::parse_path_object_id,
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use content_notice_service::use_cases::queries::get_admin_content_notice::GetAdminContentNoticeRequest;

pub(super) async fn get_admin(
    State(state): State<ContentNoticesState>,
    headers: HeaderMap,
    Path(raw_content_notice_id): Path<String>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let content_notice_id =
        match parse_path_object_id(&raw_content_notice_id, "contentNoticeId", "ContentNotice") {
            Ok(value) => value,
            Err(error) => return no_store(error.into_response()),
        };

    match state
        .get_admin
        .execute(&context, GetAdminContentNoticeRequest { content_notice_id })
        .await
    {
        Ok(details) => no_store(Json(AdminContentNoticeDetailsData::from(details)).into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

#[cfg(test)]
mod tests {
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use crate::content_notices::{router, test_support::Unused};
    use crate::state::ContentNoticesState;
    use application::{error::static_error, operation_context::OperationContext};
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
    };
    use content_notice_core::{
        additional_url::AdditionalUrls,
        content_notice::{ContentNotice, NewContentNotice},
        content_notice_id::ContentNoticeId,
        content_notice_target::{ContentNoticeTarget, ListingSourceSnapshot, TargetSnapshot},
        good_faith_statement_version::GoodFaithStatementVersion,
        reason_category::ReasonCategory,
        submitter::{Submitter, SubmitterEmail, SubmitterName},
        submitter_role::SubmitterRole,
    };
    use content_notice_service::{
        ports::ContentNoticeStorageVersion,
        use_cases::queries::get_admin_content_notice::{
            AdminContentNoticeDetails, CurrentContentNoticeTarget, GetAdminContentNoticeError,
            GetAdminContentNoticeRequest, GetAdminContentNoticeUseCase,
        },
    };
    use listing_source_core::{ListingSourceId, ListingSourceName};
    use localization::Language;
    use serde_json::{Value, json};
    use std::{
        collections::BTreeSet,
        sync::{Arc, Mutex},
    };
    use time::macros::datetime;
    use tower::ServiceExt;
    use user_core::user_id::UserId;

    #[derive(Clone, Copy)]
    enum Outcome {
        Found,
        NotFound,
        Forbidden,
        Unavailable,
        Invalid,
    }

    struct FakeGetAdmin {
        outcome: Outcome,
        requested: Mutex<Option<ContentNoticeId>>,
    }

    #[async_trait::async_trait]
    impl GetAdminContentNoticeUseCase for FakeGetAdmin {
        async fn execute(
            &self,
            _: &OperationContext,
            request: GetAdminContentNoticeRequest,
        ) -> Result<AdminContentNoticeDetails, GetAdminContentNoticeError> {
            *self.requested.lock().unwrap() = Some(request.content_notice_id);
            match self.outcome {
                Outcome::Found => Ok(details(request.content_notice_id)),
                Outcome::NotFound => Err(GetAdminContentNoticeError::NotFound),
                Outcome::Forbidden => Err(GetAdminContentNoticeError::Forbidden),
                Outcome::Unavailable => Err(GetAdminContentNoticeError::BeginTransactionFailed(
                    static_error("begin failed"),
                )),
                Outcome::Invalid => Err(GetAdminContentNoticeError::InvalidReadModel {
                    source: static_error("invalid row"),
                }),
            }
        }
    }

    fn details(id: ContentNoticeId) -> AdminContentNoticeDetails {
        AdminContentNoticeDetails {
            notice: ContentNotice::submit(NewContentNotice {
                id,
                target: ContentNoticeTarget::ListingSource {
                    listing_source_id: ListingSourceId::new(),
                },
                target_snapshot: TargetSnapshot {
                    listing_source: ListingSourceSnapshot {
                        name: ListingSourceName::try_from("Antik Shop").unwrap(),
                        url: None,
                        partner: false,
                    },
                    product_listing: None,
                },
                submitter_role: SubmitterRole::Provider,
                reason_category: ReasonCategory::ProviderOptOut,
                explanation: None,
                additional_urls: AdditionalUrls::default(),
                submitter: Submitter {
                    name: SubmitterName::try_from("Erika Mustermann").unwrap(),
                    email: SubmitterEmail::try_from("erika@antik.example").unwrap(),
                    language: Language::De,
                },
                good_faith_statement_version: GoodFaithStatementVersion::GoodFaithV1,
            })
            .unwrap(),
            version: ContentNoticeStorageVersion::default(),
            created: datetime!(2026-11-13 10:44 UTC),
            updated: datetime!(2026-11-13 10:44 UTC),
            current_target: CurrentContentNoticeTarget {
                listing_source: None,
                product_listing: None,
            },
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

    struct Response {
        status: StatusCode,
        cache_control: Option<String>,
        body: Value,
        requested: Option<ContentNoticeId>,
    }

    async fn get(outcome: Outcome, authenticated: bool, raw_id: &str) -> Response {
        let use_case = Arc::new(FakeGetAdmin {
            outcome,
            requested: Mutex::new(None),
        });
        let router = router(ContentNoticesState::new(
            Arc::new(Unused),
            Arc::new(Unused),
            Arc::clone(&use_case) as Arc<dyn GetAdminContentNoticeUseCase>,
            Arc::new(FakeAuthenticator(authenticated)),
        ));
        let request = Request::builder()
            .uri(format!("/v1/admin/content-notices/{raw_id}"))
            .header("Authorization", "Bearer test")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(request).await.unwrap();
        let status = response.status();
        let cache_control = response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let requested = *use_case.requested.lock().unwrap();
        Response {
            status,
            cache_control,
            body: serde_json::from_slice(&body).unwrap_or(Value::Null),
            requested,
        }
    }

    #[tokio::test]
    async fn should_return_details_of_the_requested_notice_and_never_store_them() {
        let id = ContentNoticeId::new();

        let response = get(Outcome::Found, true, &id.to_string()).await;

        assert_eq!(StatusCode::OK, response.status);
        assert_eq!(Some("no-store".to_owned()), response.cache_control);
        assert_eq!(Some(id), response.requested);
        assert_eq!(json!(id.to_string()), response.body["contentNoticeId"]);
        assert_eq!(
            json!({"listingSource": {"exists": false}}),
            response.body["currentTarget"]
        );
    }

    #[tokio::test]
    async fn should_reject_a_malformed_id_before_reading() {
        let response = get(Outcome::Found, true, "pl_not-a-notice").await;

        assert_eq!(StatusCode::BAD_REQUEST, response.status);
        assert_eq!(Some("no-store".to_owned()), response.cache_control);
        assert_eq!(json!("INVALID_OBJECT_ID"), response.body["error"]);
        assert_eq!(None, response.requested);
    }

    #[tokio::test]
    async fn should_map_errors_and_never_store_them() {
        for (outcome, authenticated, expected_status, expected_error) in [
            (
                Outcome::Found,
                false,
                StatusCode::UNAUTHORIZED,
                "INVALID_CREDENTIALS",
            ),
            (
                Outcome::NotFound,
                true,
                StatusCode::NOT_FOUND,
                "CONTENT_NOTICE_NOT_FOUND",
            ),
            (Outcome::Forbidden, true, StatusCode::FORBIDDEN, "FORBIDDEN"),
            (
                Outcome::Unavailable,
                true,
                StatusCode::SERVICE_UNAVAILABLE,
                "CONTENT_NOTICE_TEMPORARILY_UNAVAILABLE",
            ),
            (
                Outcome::Invalid,
                true,
                StatusCode::INTERNAL_SERVER_ERROR,
                "CONTENT_NOTICE_INTERNAL_ERROR",
            ),
        ] {
            let response = get(outcome, authenticated, &ContentNoticeId::new().to_string()).await;

            assert_eq!(expected_status, response.status, "{expected_error}");
            assert_eq!(
                Some("no-store".to_owned()),
                response.cache_control,
                "{expected_error}"
            );
            assert_eq!(json!(expected_error), response.body["error"]);
        }
    }
}
