use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use notification::service::notification_service::NotificationService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl NotificationService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());

    service.delete_all_notifications(&user_id).await?;

    Ok(ApiGatewayV2HttpResponseBuilder::json(204).build())
}

#[cfg(test)]
mod tests {
    use super::handle;
    use common::user_id::UserId;
    use lambda_runtime::LambdaEvent;
    use notification::service::notification_service::MockNotificationService;
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    async fn should_204_when_success() {
        let mut service = MockNotificationService::default();
        service
            .expect_delete_all_notifications()
            .return_once(|_| Box::pin(async { Ok(()) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap();

        assert_eq!(204, response.status_code);
    }

    #[tokio::test]
    async fn should_401_when_sub_missing() {
        let mut service = MockNotificationService::default();
        service.expect_delete_all_notifications().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &service).await.unwrap_err();
        assert_eq!(401, actual.status);
    }
}
