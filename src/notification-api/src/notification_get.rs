use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::event_id::EventId;
use common::event_id::api::extract_event_id_cursor_query;
use common::language::data::api::extract_language_query;
use common::pagination::cursor::CursoredResult;
use common::user_id::api::extract_user_id_request_context;
use lambda_runtime::LambdaEvent;
use notification::data::get_notification_data::GetNotificationData;
use notification::service::notification_service::NotificationService;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventIdCursoredData<T> {
    pub items: Vec<T>,
    pub size: u64,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_after: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

impl<T> From<CursoredResult<T, EventId>> for EventIdCursoredData<T> {
    fn from(result: CursoredResult<T, EventId>) -> Self {
        EventIdCursoredData {
            items: result.items,
            size: result.cursor.size,
            search_after: result.cursor.search_after.map(|id| id.to_string()),
            total: result.total,
        }
    }
}

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl NotificationService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    tracing::Span::current().record("userId", user_id.to_string());
    let language = extract_language_query(&event.payload.query_string_parameters)?;
    let cursor = extract_event_id_cursor_query(&event.payload.query_string_parameters)?;

    let notifications = service
        .view_notifications(&user_id, &[language.into()], &Default::default(), &cursor)
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
    use http::header::CACHE_CONTROL;
    use lambda_runtime::LambdaEvent;
    use notification::service::notification_service::MockNotificationService;
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    async fn should_200_when_success() {
        let mut service = MockNotificationService::default();
        service
            .expect_view_notifications()
            .return_once(|_, _, _, _| {
                Box::pin(async {
                    Ok(common::pagination::cursor::CursoredResult {
                        items: vec![],
                        cursor: Default::default(),
                        total: Some(0),
                    })
                })
            });

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .jwt_claim("sub", UserId::new())
                .query_string_parameter("language", "de")
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_401_when_sub_missing() {
        let mut service = MockNotificationService::default();
        service.expect_view_notifications().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .query_string_parameter("language", "de")
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &service).await.unwrap_err();
        assert_eq!(401, actual.status);
    }

    #[tokio::test]
    async fn should_set_cache_control_to_no_store() {
        let mut service = MockNotificationService::default();
        service
            .expect_view_notifications()
            .return_once(|_, _, _, _| {
                Box::pin(async {
                    Ok(common::pagination::cursor::CursoredResult {
                        items: vec![],
                        cursor: Default::default(),
                        total: Some(0),
                    })
                })
            });

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handle(lambda_event, &service).await.unwrap();

        assert_eq!(200, response.status_code);
        assert_eq!(
            "no-store",
            response
                .headers
                .get(CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap()
        );
    }
}
