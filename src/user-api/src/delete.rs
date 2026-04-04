use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use user::service::user_service::UserService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl UserService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());

    service.delete_user(&user_id).await?;

    Ok(ApiGatewayV2HttpResponseBuilder::new(204).build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use common::user_id::UserId;
    use lambda_runtime::LambdaEvent;
    use test_api::ApiGatewayV2httpRequestProxy;
    use user::service::user_service::{MockUserService, UserServiceError};

    #[tokio::test]
    async fn should_204_when_delete_succeeds() {
        let mut service = MockUserService::default();
        service
            .expect_delete_user()
            .return_once(move |_| Box::pin(async move { Ok(()) }));
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
                .route_key("DELETE /api/v1/me")
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap();

        assert_eq!(204, response.status_code);
    }

    #[tokio::test]
    async fn should_401_when_jwt_claim_sub_is_missing() {
        let service = MockUserService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
                .route_key("DELETE /api/v1/me")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap_err();

        assert_eq!(401, response.status);
    }

    #[tokio::test]
    async fn should_404_when_user_does_not_exist() {
        let mut service = MockUserService::default();
        service.expect_delete_user().return_once(move |user_id| {
            let user_id = *user_id;
            Box::pin(async move { Err(UserServiceError::UserNotFound(user_id)) })
        });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
                .route_key("DELETE /api/v1/me")
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap_err();

        assert_eq!(404, response.status);
    }

    #[tokio::test]
    async fn should_500_when_cognito_admin_service_not_configured() {
        let mut service = MockUserService::default();
        service.expect_delete_user().return_once(move |_| {
            Box::pin(async move { Err(UserServiceError::CognitoAdminServiceNotConfigured) })
        });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
                .route_key("DELETE /api/v1/me")
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap_err();

        assert_eq!(500, response.status);
    }
}
