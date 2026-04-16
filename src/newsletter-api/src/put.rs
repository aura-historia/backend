use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use cognito::access_token_verifier_service::AccessTokenVerifierService;
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::BAD_BODY_VALUE;
use common::currency::domain::Currency;
use common::language::domain::Language;
use lambda_runtime::LambdaEvent;

use crate::data::PutNewsletterSubscriptionData;
use crate::domain::UpsertNewsletterSubscription;
use crate::service::{ZohoCampaignsError, ZohoCampaignsService};

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    zoho_campaigns_service: &(impl ZohoCampaignsService + Sync),
    access_token_verifier_service: &(impl AccessTokenVerifierService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id_opt = access_token_verifier_service
        .verify_extract_user_id(&event.payload.headers)
        .await?;
    if let Some(user_id) = user_id_opt {
        tracing::Span::current().record("userId", user_id.to_string());
    }

    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            let err_msg = "Body cannot be empty";
            ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_detail(err_msg)
        })?;

    let put_data: PutNewsletterSubscriptionData = serde_json::from_str(&body).map_err(|err| {
        let err_msg = err.to_string();
        ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
    })?;

    let subscription = UpsertNewsletterSubscription {
        email: put_data.email,
        first_name: put_data.first_name,
        last_name: put_data.last_name,
        language: put_data.language.map(Language::from),
        currency: put_data.currency.map(Currency::from),
        user_id: user_id_opt,
        tier: None,
    };

    zoho_campaigns_service
        .subscribe(&subscription)
        .await
        .map_err(|e| match e {
            ZohoCampaignsError::OAuthTokenError(_)
            | ZohoCampaignsError::ApiRequestError(_)
            | ZohoCampaignsError::ApiResponseError { .. } => ApiError::internal_server_error(
                common::api::error_code::INTERNAL_SERVER_ERROR,
                Box::new(e),
            ),
        })?;

    Ok(ApiGatewayV2HttpResponseBuilder::new(204).build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use crate::service::MockZohoCampaignsService;
    use cognito::access_token_verifier_service::MockAccessTokenVerifierService;
    use common::user_id::UserId;
    use lambda_runtime::LambdaEvent;
    use test_api::ApiGatewayV2httpRequestProxy;

    use crate::data::PutNewsletterSubscriptionData;

    #[tokio::test]
    async fn should_return_204_when_subscription_succeeds() {
        let mut zoho_service = MockZohoCampaignsService::default();
        zoho_service
            .expect_subscribe()
            .return_once(|_| Box::pin(async { Ok(()) }));

        let mut access_token_service = MockAccessTokenVerifierService::default();
        access_token_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));

        let data = PutNewsletterSubscriptionData {
            email: "test@example.com".try_into().unwrap(),
            first_name: Some("Test".into()),
            last_name: Some("User".into()),
            language: None,
            currency: None,
        };

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PUT)
                .route_key("PUT /api/v1/newsletter-subscriptions")
                .body_serde(&data)
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &zoho_service, &access_token_service)
            .await
            .unwrap();
        assert_eq!(204, response.status_code);
    }

    #[tokio::test]
    async fn should_pass_user_id_when_authenticated() {
        let user_id = UserId::new();
        let expected_user_id = user_id;

        let mut zoho_service = MockZohoCampaignsService::default();
        zoho_service
            .expect_subscribe()
            .withf(move |sub| sub.user_id == Some(expected_user_id))
            .return_once(|_| Box::pin(async { Ok(()) }));

        let mut access_token_service = MockAccessTokenVerifierService::default();
        access_token_service
            .expect_verify_extract_user_id()
            .return_once(move |_| Box::pin(async move { Ok(Some(user_id)) }));

        let data = PutNewsletterSubscriptionData {
            email: "auth@example.com".try_into().unwrap(),
            first_name: None,
            last_name: None,
            language: None,
            currency: None,
        };

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PUT)
                .route_key("PUT /api/v1/newsletter-subscriptions")
                .body_serde(&data)
                .jwt_claim("sub", user_id)
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &zoho_service, &access_token_service)
            .await
            .unwrap();
        assert_eq!(204, response.status_code);
    }

    #[tokio::test]
    async fn should_pass_none_user_id_when_unauthenticated() {
        let mut zoho_service = MockZohoCampaignsService::default();
        zoho_service
            .expect_subscribe()
            .withf(|sub| sub.user_id.is_none())
            .return_once(|_| Box::pin(async { Ok(()) }));

        let mut access_token_service = MockAccessTokenVerifierService::default();
        access_token_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));

        let data = PutNewsletterSubscriptionData {
            email: "anon@example.com".try_into().unwrap(),
            first_name: None,
            last_name: None,
            language: None,
            currency: None,
        };

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PUT)
                .route_key("PUT /api/v1/newsletter-subscriptions")
                .body_serde(&data)
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &zoho_service, &access_token_service)
            .await
            .unwrap();
        assert_eq!(204, response.status_code);
    }

    #[tokio::test]
    async fn should_return_400_when_body_is_empty() {
        let zoho_service = MockZohoCampaignsService::default();
        let mut access_token_service = MockAccessTokenVerifierService::default();
        access_token_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PUT)
                .route_key("PUT /api/v1/newsletter-subscriptions")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &zoho_service, &access_token_service).await;
        assert!(response.is_err());
        assert_eq!(400, response.unwrap_err().status);
    }

    #[tokio::test]
    async fn should_return_400_when_body_is_invalid_json() {
        let zoho_service = MockZohoCampaignsService::default();
        let mut access_token_service = MockAccessTokenVerifierService::default();
        access_token_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PUT)
                .route_key("PUT /api/v1/newsletter-subscriptions")
                .body_serde(&"not a valid newsletter body")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &zoho_service, &access_token_service).await;
        assert!(response.is_err());
        assert_eq!(400, response.unwrap_err().status);
    }

    #[tokio::test]
    async fn should_return_500_when_zoho_service_fails() {
        use crate::service::ZohoCampaignsError;

        let mut zoho_service = MockZohoCampaignsService::default();
        zoho_service.expect_subscribe().return_once(|_| {
            Box::pin(async {
                Err(ZohoCampaignsError::ApiResponseError {
                    status: "error".to_string(),
                    message: "Some Zoho error".to_string(),
                })
            })
        });

        let mut access_token_service = MockAccessTokenVerifierService::default();
        access_token_service
            .expect_verify_extract_user_id()
            .return_once(|_| Box::pin(async { Ok(None) }));

        let data = PutNewsletterSubscriptionData {
            email: "fail@example.com".try_into().unwrap(),
            first_name: None,
            last_name: None,
            language: None,
            currency: None,
        };

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PUT)
                .route_key("PUT /api/v1/newsletter-subscriptions")
                .body_serde(&data)
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &zoho_service, &access_token_service).await;
        assert!(response.is_err());
        assert_eq!(500, response.unwrap_err().status);
    }
}
