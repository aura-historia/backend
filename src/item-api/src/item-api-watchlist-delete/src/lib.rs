use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_logging::log_api_error;
use common::shop_id::api::extract_shop_id_path;
use common::shops_item_id::api::extract_shops_item_id_path;
use common::user_id::api::extract_user_id_request_context;
use item::watchlist::service::item_watchlist_service::ItemWatchListService;
use lambda_runtime::LambdaEvent;

#[tracing::instrument(
    skip(event, service),
    fields(
        requestId = %event.context.request_id,
        path = &event.payload.raw_path,
        query = &event.payload.raw_query_string,
        method = %event.payload.http_method,
        userId = tracing::field::Empty,
        clientIp = tracing::field::Empty,
    )
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl ItemWatchListService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    // Extract and record user ID if available
    if let Some(user_id) = event
        .payload
        .request_context
        .authorizer
        .as_ref()
        .and_then(|auth| auth.jwt.as_ref())
        .and_then(|jwt| jwt.claims.get("sub"))
    {
        tracing::Span::current().record("userId", user_id);
    } else {
        tracing::Span::current().record("userId", "anonymous");
    }

    // Extract and record client IP if available
    if let Some(source_ip) = event.payload.request_context.http.source_ip.as_ref() {
        tracing::Span::current().record("clientIp", source_ip.as_str());
    }

    match handle(event, service).await {
        Ok(response) => Ok(response),
        Err(err) => {
            log_api_error(&err);
            Ok(ApiGatewayV2httpResponse::from(err))
        }
    }
}

// DELETE /api/v1/me/watchlist/{shopId}/{shopsItemId}
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl ItemWatchListService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_request_context(&event.payload.request_context)?;
    let shop_id = extract_shop_id_path(&event.payload.path_parameters)?;
    let shops_item_id = extract_shops_item_id_path(&event.payload.path_parameters)?;

    let () = service
        .delete_watchlist_item(&user_id, &shop_id, &shops_item_id)
        .await?;

    Ok(ApiGatewayV2HttpResponseBuilder::json(204).build())
}

#[cfg(test)]
mod tests {
    use crate::handler;
    use common::{shop_id::ShopId, shops_item_id::ShopsItemId, user_id::UserId};
    use item::watchlist::service::item_watchlist_service::MockItemWatchListService;
    use lambda_runtime::LambdaEvent;
    use test_api::{ApiGatewayV2httpRequestProxy, extract_apigw_response_json_body};
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    #[tokio::test]
    async fn should_204_when_success() {
        let mut service = MockItemWatchListService::default();
        service
            .expect_delete_watchlist_item()
            .return_once(|_, _, _| Box::pin(async { Ok(()) }));

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsItemId", ShopsItemId::new())
                .query_string_parameter(
                    "created",
                    OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
                )
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(204, response.status_code);
    }

    #[tokio::test]
    async fn should_400_when_shop_id_missing() {
        let mut service = MockItemWatchListService::default();
        service.expect_delete_watchlist_item().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
                .path_parameter("shopsItemId", ShopsItemId::new())
                .query_string_parameter(
                    "created",
                    OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
                )
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(400, response.status_code);

        let json = extract_apigw_response_json_body!(response);
        assert_eq!("BAD_PATH_PARAMETER_VALUE", json["error"]);
        assert_eq!("shopId", json["source"]["field"]);
        assert_eq!("PATH", json["source"]["type"]);
    }

    #[tokio::test]
    async fn should_400_when_shops_item_id_missing() {
        let mut service = MockItemWatchListService::default();
        service.expect_delete_watchlist_item().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
                .path_parameter("shopId", ShopId::new())
                .query_string_parameter(
                    "created",
                    OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
                )
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(400, response.status_code);

        let json = extract_apigw_response_json_body!(response);
        assert_eq!("BAD_PATH_PARAMETER_VALUE", json["error"]);
        assert_eq!("shopsItemId", json["source"]["field"]);
        assert_eq!("PATH", json["source"]["type"]);
    }

    #[tokio::test]
    async fn should_401_when_sub_missing() {
        let mut service = MockItemWatchListService::default();
        service.expect_delete_watchlist_item().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsItemId", ShopsItemId::new())
                .query_string_parameter(
                    "created",
                    OffsetDateTime::now_utc().format(&Rfc3339).unwrap(),
                )
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();

        assert_eq!(401, response.status_code);
    }
}
