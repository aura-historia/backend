//! Internal routes for AWS components that stay outside the host. They are not part of the
//! public API contract (`docs/swagger.yaml`); each authenticates its caller itself, and
//! `infra/test/api-route-parity.test.ts` lists them.
use crate::state::CognitoPostConfirmationState;
use axum::{Router, routing::post};

pub mod cognito_post_confirmation;

pub(crate) fn routes(state: CognitoPostConfirmationState) -> Router {
    Router::new()
        .route(
            "/internal/cognito/post-confirmation",
            post(cognito_post_confirmation::post_cognito_post_confirmation).layer(
                tower_http::limit::RequestBodyLimitLayer::new(
                    cognito_post_confirmation::MAX_BODY_BYTES,
                ),
            ),
        )
        .with_state(state)
}
