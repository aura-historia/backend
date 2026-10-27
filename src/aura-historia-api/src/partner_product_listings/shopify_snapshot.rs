use super::{async_batch, types::parse_listing_source_id};
use crate::{
    error::{
        ApiError, BAD_BODY_VALUE, FORBIDDEN, PRODUCT_LISTING_INTERNAL_ERROR,
        PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE,
    },
    state::AsyncPartnerProductListingsState,
};
use aura_historia_contracts::partner_product_listings::shopify_snapshot::{
    ShopifySnapshotData, ShopifySnapshotOperation,
};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use listing_source_core::Domain;
use product_listing_service::{
    ports::shopify_product_decoder::ShopifyProductEventKind,
    use_cases::commands::submit_shopify_snapshot::{
        ShopifySnapshotItem, SubmitShopifySnapshotCommand, SubmitShopifySnapshotError,
    },
};

pub async fn submit_snapshot(
    State(state): State<AsyncPartnerProductListingsState>,
    headers: HeaderMap,
    Path(raw_listing_source_id): Path<String>,
    body: String,
) -> Response {
    let context =
        match async_batch::authorized_context(state.authenticator.as_ref(), &headers).await {
            Ok(context) => context,
            Err(response) => return response,
        };
    let listing_source_id = match parse_listing_source_id(&raw_listing_source_id) {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let key = match async_batch::idempotency_key(&headers) {
        Ok(Some(key)) => key,
        Ok(None) => {
            return ApiError::bad_request(BAD_BODY_VALUE)
                .with_detail("Idempotency-Key is required.")
                .into_response();
        }
        Err(error) => return error.into_response(),
    };
    let data = match serde_json::from_str::<ShopifySnapshotData>(&body) {
        Ok(data) => data,
        Err(_) => return ApiError::bad_request(BAD_BODY_VALUE).into_response(),
    };
    let domain = match Domain::try_from(data.domain) {
        Ok(domain) => domain,
        Err(_) => return ApiError::bad_request(BAD_BODY_VALUE).into_response(),
    };
    let mut items = Vec::with_capacity(data.items.len());
    for item in data.items {
        let payload = match product_listing_normalization::SourcePayload::new(item.payload) {
            Ok(payload) => payload,
            Err(_) => return ApiError::bad_request(BAD_BODY_VALUE).into_response(),
        };
        items.push(ShopifySnapshotItem {
            kind: match item.operation {
                ShopifySnapshotOperation::Upsert => ShopifyProductEventKind::Add,
                ShopifySnapshotOperation::Withdraw => ShopifyProductEventKind::Remove,
            },
            payload,
        });
    }
    let Some(submit) = state.shopify_snapshots else {
        return ApiError::service_unavailable(PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE)
            .into_response();
    };
    match submit
        .execute(
            &context,
            SubmitShopifySnapshotCommand {
                listing_source_id,
                domain,
                snapshot_started_at: data.snapshot_started_at,
                selection_complete: data.selection_complete,
                idempotency_key: key,
                items,
            },
        )
        .await
    {
        Ok(result) => {
            let source_ids = vec![None; result.original_input_count];
            async_batch::report(result, vec![], &source_ids)
        }
        Err(SubmitShopifySnapshotError::Submission(error)) => async_batch::submission_error(error),
        Err(error) => match error {
            SubmitShopifySnapshotError::Forbidden | SubmitShopifySnapshotError::SourceMismatch => {
                ApiError::forbidden(FORBIDDEN)
            }
            SubmitShopifySnapshotError::IncompleteSelection
            | SubmitShopifySnapshotError::InvalidPayload(_) => {
                ApiError::bad_request(BAD_BODY_VALUE)
            }
            SubmitShopifySnapshotError::SourceLookup(
                listing_source_service::ports::ListingSourceReadError::TemporarilyUnavailable {
                    ..
                },
            ) => ApiError::service_unavailable(PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE),
            _ => ApiError::internal_server_error(PRODUCT_LISTING_INTERNAL_ERROR),
        }
        .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use application::operation_context::{CredentialCapability, OperationContext};
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode},
        routing::post,
    };
    use product_listing_service::use_cases::commands::submit_shopify_snapshot::SubmitShopifySnapshotUseCase;
    use product_listing_service::use_cases::{
        ProductListingIngestionSubmission, ProductListingIngestionSubmissionError,
        ProductListingIngestionSubmissionResult, SubmitPartnerProductListingIngestionUseCase,
    };
    use std::{
        collections::BTreeSet,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use tower::ServiceExt;

    struct Auth(bool);
    #[async_trait::async_trait]
    impl TokenAuthenticator for Auth {
        async fn authenticate(
            &self,
            token: &str,
            _: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            if token != "valid" {
                return Err(AuthError::InvalidCredentials);
            }
            Ok(TransportPrincipal::User {
                user_id: user_core::user_id::UserId::new(),
                auth_method: AuthMethod::AuraAccessToken,
                capabilities: if self.0 {
                    BTreeSet::from([CredentialCapability::ProductListingsWrite])
                } else {
                    BTreeSet::new()
                },
            })
        }
    }
    struct UnusedIntake;
    #[async_trait::async_trait]
    impl SubmitPartnerProductListingIngestionUseCase for UnusedIntake {
        async fn execute(
            &self,
            _: &OperationContext,
            _: ProductListingIngestionSubmission,
        ) -> Result<ProductListingIngestionSubmissionResult, ProductListingIngestionSubmissionError>
        {
            panic!("snapshot must use its own use case");
        }
    }
    struct Snapshot(Arc<AtomicUsize>);
    #[async_trait::async_trait]
    impl SubmitShopifySnapshotUseCase for Snapshot {
        async fn execute(
            &self,
            _: &OperationContext,
            command: SubmitShopifySnapshotCommand,
        ) -> Result<ProductListingIngestionSubmissionResult, SubmitShopifySnapshotError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            assert_eq!("partner.example", command.domain.as_str());
            assert_eq!("stable-key", command.idempotency_key.as_str());
            assert!(command.selection_complete);
            assert_eq!(1, command.items.len());
            assert_eq!(ShopifyProductEventKind::Remove, command.items[0].kind);
            assert_eq!(
                serde_json::json!(42),
                command.items[0].payload.value()["product_listing"]["product_id"]
            );
            Err(SubmitShopifySnapshotError::SourceMismatch)
        }
    }
    async fn invoke(
        body: &str,
        key: bool,
        writer: bool,
        token: &str,
        calls: Arc<AtomicUsize>,
    ) -> Response {
        let state =
            AsyncPartnerProductListingsState::new(Arc::new(UnusedIntake), Arc::new(Auth(writer)))
                .with_shopify_snapshots(Arc::new(Snapshot(calls)));
        let app = Router::new()
            .route("/{source}", post(submit_snapshot))
            .with_state(state);
        let mut request = Request::builder()
            .method("POST")
            .uri(format!("/{}", listing_source_core::ListingSourceId::new()))
            .header("Authorization", format!("Bearer {token}"));
        if key {
            request = request.header("Idempotency-Key", "stable-key");
        }
        app.oneshot(request.body(Body::from(body.to_owned())).unwrap())
            .await
            .unwrap()
    }
    fn body() -> String {
        serde_json::json!({"domain":"partner.example","snapshotStartedAt":"2026-01-01T00:00:00Z","selectionComplete":true,"items":[{"operation":"WITHDRAW","payload":{"product_listing":{"product_id":42}}}]}).to_string()
    }

    #[tokio::test]
    async fn should_map_snapshot_contract_and_source_authority_failure() {
        let calls = Arc::new(AtomicUsize::new(0));
        let response = invoke(&body(), true, true, "valid", calls.clone()).await;
        assert_eq!(StatusCode::FORBIDDEN, response.status());
        assert_eq!(1, calls.load(Ordering::SeqCst));
        let response: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!("FORBIDDEN", response["error"]);
    }
    #[tokio::test]
    async fn should_reject_invalid_metadata_and_identity_before_snapshot_admission() {
        let calls = Arc::new(AtomicUsize::new(0));
        for (body, key, writer, token, status) in [
            (body(), false, true, "valid", StatusCode::BAD_REQUEST),
            (
                body().replace("2026-01-01T00:00:00Z", "malformed"),
                true,
                true,
                "valid",
                StatusCode::BAD_REQUEST,
            ),
            (
                body().replace("\"selectionComplete\":true,", ""),
                true,
                true,
                "valid",
                StatusCode::BAD_REQUEST,
            ),
            (body(), true, false, "valid", StatusCode::FORBIDDEN),
            (body(), true, true, "invalid", StatusCode::UNAUTHORIZED),
        ] {
            assert_eq!(
                status,
                invoke(&body, key, writer, token, calls.clone())
                    .await
                    .status()
            );
        }
        assert_eq!(0, calls.load(Ordering::SeqCst));
    }
}
