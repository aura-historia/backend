use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::BAD_BODY_VALUE;
use common::event_id::EventId;
use common::language::data::api::extract_language_query;
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use notification::data::get_notification_data::GetNotificationData;
use notification::service::notification_service::NotificationService;
use notification_get::EventIdCursoredData;
use serde::{Deserialize, Serialize};

use crate::notification_get;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkAllSeenData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_ids: Option<Vec<EventId>>,
}

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl NotificationService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());
    let language = extract_language_query(&event.payload.query_string_parameters)?;

    let event_ids: Option<Vec<EventId>> = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .map(|body| {
            serde_json::from_str::<MarkAllSeenData>(&body).map_err(|err| {
                let err_msg = err.to_string();
                ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
            })
        })
        .transpose()?
        .and_then(|data| data.event_ids);

    let event_ids_slice: Option<Vec<EventId>> = event_ids;

    let notifications = service
        .mark_all_notifications_seen(
            &user_id,
            event_ids_slice.as_deref(),
            &[language.into()],
            &Default::default(),
        )
        .await?
        .map_item(GetNotificationData::from);

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
    use notification::service::notification_service::MockNotificationService;
    use test_api::ApiGatewayV2httpRequestProxy;

    fn empty_result() -> common::pagination::cursor::CursoredResult<
        notification::core::notification::LocalizedNotification,
        common::event_id::EventId,
    > {
        common::pagination::cursor::CursoredResult {
            items: vec![],
            cursor: Default::default(),
            total: Some(0),
        }
    }

    #[tokio::test]
    async fn should_200_when_success_without_body() {
        let mut service = MockNotificationService::default();
        service
            .expect_mark_all_notifications_seen()
            .return_once(|_, _, _, _| Box::pin(async { Ok(empty_result()) }));

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
    async fn should_200_when_success_with_event_ids() {
        let mut service = MockNotificationService::default();
        service
            .expect_mark_all_notifications_seen()
            .return_once(|_, _, _, _| Box::pin(async { Ok(empty_result()) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .jwt_claim("sub", UserId::new())
                .body_serde(&super::MarkAllSeenData {
                    event_ids: Some(vec![]),
                })
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_400_when_body_invalid() {
        let mut service = MockNotificationService::default();
        service.expect_mark_all_notifications_seen().never();

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
        service.expect_mark_all_notifications_seen().never();

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
