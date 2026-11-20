//! Internal routes for AWS components that stay outside the host. They are not part of the
//! public API contract (`docs/swagger.yaml`); each authenticates its caller itself, and
//! `infra/test/api-route-parity.test.ts` lists them.
use crate::state::{CognitoPostConfirmationState, CognitoPreSignUpState};
use axum::http::HeaderMap;
use axum::{Router, routing::post};

pub mod cognito_post_confirmation;
pub mod cognito_pre_sign_up;

pub(crate) fn post_confirmation_routes(state: CognitoPostConfirmationState) -> Router {
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

pub(crate) fn pre_sign_up_routes(state: CognitoPreSignUpState) -> Router {
    Router::new()
        .route(
            "/internal/cognito/pre-sign-up",
            post(cognito_pre_sign_up::post_cognito_pre_sign_up).layer(
                tower_http::limit::RequestBodyLimitLayer::new(cognito_pre_sign_up::MAX_BODY_BYTES),
            ),
        )
        .with_state(state)
}

/// The header's value only when it is present exactly once.
fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a [u8]> {
    let mut values = headers.get_all(name).iter();
    match (values.next(), values.next()) {
        (Some(value), None) => Some(value.as_bytes()),
        _ => None,
    }
}
