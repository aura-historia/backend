use crate::notification_get::EventIdCursoredData;
use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::BAD_BODY_VALUE;
use common::currency::data::api::extract_currency_query;
use common::language::data::api::extract_language_query;
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use notification::data::get_notification_data::GetNotificationData;
use notification::data::patch_notification_data::PatchNotificationData;
use notification::service::notification_service::NotificationService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl NotificationService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());
    let language = extract_language_query(&event.payload.query_string_parameters)?;
    let currency = extract_currency_query(&event.payload.query_string_parameters)?;

    let patch: PatchNotificationData = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .map(|body| {
            serde_json::from_str::<PatchNotificationData>(&body).map_err(|err| {
                let err_msg = err.to_string();
                ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
            })
        })
        .transpose()?
        .unwrap_or_default();

    let notifications = service
        .update_notifications(&user_id, patch.into())
        .await?
        .map_item(|n| {
            let localized = n.localized(&currency.into(), &[language.into()]);
            GetNotificationData::from(localized)
        });

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .cache_control("no-store", None, None)
        .body_serde(EventIdCursoredData::from(notifications))?
        .build())
}

#[cfg(test)]
mod tests {
    use super::handle;
    use common::user_id::UserId;
    use lambda_runtime::LambdaEvent;
    use notification::core::notification::Notification;
    use notification::data::patch_notification_data::PatchNotificationData;
    use notification::dynamodb::notification_record::NotificationRecord;
    use notification::service::notification_service::MockNotificationService;
    use test_api::ApiGatewayV2httpRequestProxy;

    fn empty_result()
    -> common::pagination::cursor::CursoredResult<Notification, common::event_id::EventId> {
        common::pagination::cursor::CursoredResult {
            items: vec![],
            cursor: Default::default(),
            total: Some(0),
        }
    }

    fn one_result(
        record: NotificationRecord,
    ) -> common::pagination::cursor::CursoredResult<Notification, common::event_id::EventId> {
        common::pagination::cursor::CursoredResult {
            items: vec![record.into()],
            cursor: Default::default(),
            total: Some(1),
        }
    }

    #[tokio::test]
    async fn should_200_when_success_without_body() {
        let mut service = MockNotificationService::default();
        service
            .expect_update_notifications()
            .return_once(|_, _| Box::pin(async { Ok(empty_result()) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_200_when_success_with_body() {
        let mut service = MockNotificationService::default();
        service
            .expect_update_notifications()
            .return_once(|_, _| Box::pin(async { Ok(empty_result()) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .jwt_claim("sub", UserId::new())
                .body_serde(&PatchNotificationData { seen: Some(true) })
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_200_with_localized_items() {
        use fake::{Fake, Faker};
        let mut record: NotificationRecord = Faker.fake();
        // Use a fixed, valid timestamp so serialization doesn't fail
        record.created = time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        record.updated = time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();

        let mut service = MockNotificationService::default();
        service
            .expect_update_notifications()
            .return_once(move |_, _| Box::pin(async move { Ok(one_result(record)) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .jwt_claim("sub", UserId::new())
                .query_string_parameter("language", "de")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_400_when_body_invalid() {
        let mut service = MockNotificationService::default();
        service.expect_update_notifications().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .jwt_claim("sub", UserId::new())
                .body_serde(&"not-an-object")
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &service).await.unwrap_err();
        assert_eq!(400, actual.status);
    }

    #[tokio::test]
    async fn should_401_when_sub_missing() {
        let mut service = MockNotificationService::default();
        service.expect_update_notifications().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &service).await.unwrap_err();
        assert_eq!(401, actual.status);
    }
}
