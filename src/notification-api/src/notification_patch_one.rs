use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::BAD_BODY_VALUE;
use common::event_id::api::extract_event_id_path;
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use notification::data::get_notification_data::GetNotificationData;
use notification::data::patch_notification_data::PatchNotificationData;
use notification::service::command::UpdateNotificationCommand;
use notification::service::notification_service::NotificationService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl NotificationService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());
    let event_id = extract_event_id_path(&event.payload.path_parameters)?;
    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            let err_msg = "Body cannot be empty";
            ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_detail(err_msg)
        })?;
    let patch: PatchNotificationData = serde_json::from_str(&body).map_err(|err| {
        let err_msg = err.to_string();
        ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
    })?;

    let notification = service
        .update_notification(
            &user_id,
            &event_id,
            UpdateNotificationCommand {
                seen: Some(patch.seen),
            },
        )
        .await?;

    let language = Default::default();
    let currency = Default::default();
    let localized = notification.localized(&currency, &[language]);

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body_serde(GetNotificationData::from(localized))?
        .build())
}

#[cfg(test)]
mod tests {
    use super::handle;
    use common::{event_id::EventId, user_id::UserId};
    use fake::{Fake, Faker};
    use lambda_runtime::LambdaEvent;
    use notification::data::patch_notification_data::PatchNotificationData;
    use notification::dynamodb::notification_record::NotificationRecord;
    use notification::service::notification_service::MockNotificationService;
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    async fn should_200_when_success() {
        let mut service = MockNotificationService::default();
        service.expect_update_notification().return_once(|_, _, _| {
            Box::pin(async { Ok(Faker.fake::<NotificationRecord>().into()) })
        });

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("eventId", EventId::new())
                .body_serde(&PatchNotificationData { seen: true })
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_400_when_event_id_missing() {
        let mut service = MockNotificationService::default();
        service.expect_update_notification().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .body_serde(&PatchNotificationData { seen: true })
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &service).await.unwrap_err();
        assert_eq!(400, actual.status);
    }

    #[tokio::test]
    async fn should_400_when_body_missing() {
        let mut service = MockNotificationService::default();
        service.expect_update_notification().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("eventId", EventId::new())
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &service).await.unwrap_err();
        assert_eq!(400, actual.status);
    }

    #[tokio::test]
    async fn should_400_when_body_invalid() {
        let mut service = MockNotificationService::default();
        service.expect_update_notification().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("eventId", EventId::new())
                .body_serde(&"invalid")
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &service).await.unwrap_err();
        assert_eq!(400, actual.status);
    }

    #[tokio::test]
    async fn should_401_when_sub_missing() {
        let mut service = MockNotificationService::default();
        service.expect_update_notification().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .path_parameter("eventId", EventId::new())
                .body_serde(&PatchNotificationData { seen: false })
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &service).await.unwrap_err();
        assert_eq!(401, actual.status);
    }
}
