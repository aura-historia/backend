use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::{
    error::{ApiError, log_api_error},
    error_code::INTERNAL_SERVER_ERROR,
};
use lambda_runtime::LambdaEvent;
use user::service::user_service::UserService;

use crate::service::StripeService;

pub mod checkout;
pub mod portal;
pub mod service;

/// Whether the lambda runs against Stripe's live API (`prod` stage) or the
/// test API (any other stage). The frontend uses this to decide whether to
/// surface real-billing UX.
#[derive(Debug, Clone, Copy)]
pub struct LiveMode(pub bool);

impl LiveMode {
    /// Resolves [`LiveMode`] from the `STAGE` env-var. Only `STAGE=prod`
    /// returns `true`; missing or any other value returns `false`.
    pub fn from_stage(stage: Option<&str>) -> Self {
        Self(matches!(stage, Some("prod")))
    }
}

#[tracing::instrument(
    skip(event, stripe_service, user_service, live_mode),
    fields(
        requestId = %event.context.request_id,
        method = event.payload.request_context.http.method.as_str(),
        path = &event.payload.raw_path.as_deref().unwrap_or("NULL"),
        ip = &event.payload.request_context.http.source_ip.as_deref().unwrap_or("NULL"),
        userAgent = &event.payload.request_context.http.user_agent.as_deref().unwrap_or("NULL"),
        userId = tracing::field::Empty,
    )
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    stripe_service: &impl StripeService,
    user_service: &(impl UserService + Sync),
    live_mode: LiveMode,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, stripe_service, user_service, live_mode).await {
        Ok(response) => Ok(response),
        Err(err) => {
            log_api_error(&err);
            Ok(ApiGatewayV2httpResponse::from(err))
        }
    }
}

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    stripe_service: &impl StripeService,
    user_service: &(impl UserService + Sync),
    live_mode: LiveMode,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    match event.payload.route_key.as_deref() {
        Some("POST /api/v1/me/billing/checkout") => {
            checkout::handle(event, stripe_service, user_service, live_mode).await
        }
        Some("POST /api/v1/me/billing/portal") => {
            portal::handle(event, stripe_service, user_service, live_mode).await
        }
        Some(unknown) => Err(ApiError::internal_server_error(
            INTERNAL_SERVER_ERROR,
            format!("Unknown route-key '{unknown}' in AWS-Payload").into(),
        )),
        None => Err(ApiError::internal_server_error(
            INTERNAL_SERVER_ERROR,
            "Missing route-key in AWS-Payload".into(),
        )),
    }
}
