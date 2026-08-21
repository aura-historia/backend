use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use cognito::access_token_verifier_service::{
    AccessTokenVerifierError, AccessTokenVerifierService,
};
use common::api::{
    error::{ApiError, log_api_error},
    error_code::INTERNAL_SERVER_ERROR,
};
use http::header::AUTHORIZATION;
use lambda_runtime::LambdaEvent;
use user::service::user_service::UserService;

pub mod data;
pub mod domain;
pub mod put;
pub mod service;

pub(crate) fn map_access_token_error(value: AccessTokenVerifierError) -> ApiError {
    match value {
        AccessTokenVerifierError::HttpHeaderValueToStrError(ref error) => {
            let detail = error.to_string();
            ApiError::bad_request(common::api::error_code::BAD_HEADER_VALUE, Box::new(value))
                .with_header_field(AUTHORIZATION.as_str())
                .with_detail(detail)
        }
        AccessTokenVerifierError::JwtCognito(_)
        | AccessTokenVerifierError::JwtError(_)
        | AccessTokenVerifierError::JwksFetchError(_)
        | AccessTokenVerifierError::ClaimIsNotString(_) => {
            ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(value))
        }
        AccessTokenVerifierError::MissingClaim(claim) => {
            ApiError::bad_request(common::api::error_code::BAD_HEADER_VALUE, Box::new(value))
                .with_header_field(AUTHORIZATION.as_str())
                .with_detail(format!("Missing claim '{claim}'."))
        }
        AccessTokenVerifierError::InvalidUuid(claim, _) => {
            ApiError::bad_request(common::api::error_code::INVALID_UUID, Box::new(value))
                .with_detail(format!(
                    "String-Value for decoded claim '{claim}' is not a valid UUID."
                ))
        }
    }
}

#[tracing::instrument(
    skip(event, zoho_campaigns_service, access_token_verifier_service, user_service),
    fields(
        requestId = %event.context.request_id,
        method = event.payload.request_context.http.method.as_str(),
        path = &event.payload.raw_path.as_deref().unwrap_or("NULL"),
        query = &event.payload.raw_query_string.as_deref().unwrap_or("NULL"),
        body = &event.payload.body.as_deref().unwrap_or("NULL"),
        ip = &event.payload.request_context.http.source_ip.as_deref().unwrap_or("NULL"),
        userAgent = &event.payload.request_context.http.user_agent.as_deref().unwrap_or("NULL"),
        userId = tracing::field::Empty,
    )
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    zoho_campaigns_service: &(impl service::ZohoCampaignsService + Sync),
    access_token_verifier_service: &(impl AccessTokenVerifierService + Sync),
    user_service: &(impl UserService + Sync),
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(
        event,
        zoho_campaigns_service,
        access_token_verifier_service,
        user_service,
    )
    .await
    {
        Ok(response) => Ok(response),
        Err(err) => {
            log_api_error(&err);
            Ok(ApiGatewayV2httpResponse::from(err))
        }
    }
}

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    zoho_campaigns_service: &(impl service::ZohoCampaignsService + Sync),
    access_token_verifier_service: &(impl AccessTokenVerifierService + Sync),
    user_service: &(impl UserService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    match event.payload.route_key.as_deref() {
        Some("PUT /api/v1/newsletter-subscriptions") => {
            put::handle(
                event,
                zoho_campaigns_service,
                access_token_verifier_service,
                user_service,
            )
            .await
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
