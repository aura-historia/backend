use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::{BAD_QUERY_PARAMETER_VALUE, INVALID_RFC3339_TIMESTAMP};
use common::shop_id::api::extract_shop_id_path;
use common::shops_item_id::api::extract_shops_item_id_path;
use common::user_id::api::extract_user_id_cognito_jwt;
use item_watchlist::service::ItemWatchListService;
use lambda_runtime::LambdaEvent;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[tracing::instrument(
    skip(event, service),
    fields(
        requestId = %event.context.request_id,
        path = &event.payload.raw_path,
        query = &event.payload.raw_query_string,
    )
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl ItemWatchListService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, service).await {
        Ok(response) => Ok(response),
        Err(err) => Ok(ApiGatewayV2httpResponse::from(err)),
    }
}

// DELETE /api/v1/watchlist/{shopId}/{shopsItemId}?created={timestamp}
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl ItemWatchListService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let user_id = extract_user_id_cognito_jwt(&event.payload.request_context)?;
    let _ = extract_shop_id_path(&event.payload.path_parameters)?;
    let _ = extract_shops_item_id_path(&event.payload.path_parameters)?;
    let created = event
        .payload
        .query_string_parameters
        .first("created")
        .filter(|val| !val.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                .with_query_field("created")
                .with_message("The timestamp of when the watchlist-entry was created is required to delete it.")
        })
        .map(|val| OffsetDateTime::parse(val, &Rfc3339))?
        .map_err(|err| ApiError::bad_request(INVALID_RFC3339_TIMESTAMP).with_query_field("created").with_message(err.to_string()))?;

    let () = service.unwatch(&user_id, &created).await?;

    Ok(ApiGatewayV2HttpResponseBuilder::json(204).cors().build())
}

#[cfg(test)]
mod tests {
    use crate::handler;
    use common::{shop_id::ShopId, shops_item_id::ShopsItemId, user_id::UserId};
    use item_watchlist::service::MockItemWatchListService;
    use lambda_runtime::LambdaEvent;
    use test_api::{ApiGatewayV2httpRequestProxy, extract_apigw_response_json_body};
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    #[tokio::test]
    async fn should_204_when_success() {
        let mut service = MockItemWatchListService::default();
        service
            .expect_unwatch()
            .return_once(|_, _| Box::pin(async { Ok(()) }));

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
        service.expect_unwatch().never();

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
        service.expect_unwatch().never();

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
    async fn should_400_when_created_missing() {
        let mut service = MockItemWatchListService::default();
        service.expect_unwatch().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsItemId", ShopsItemId::new())
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(400, response.status_code);

        let json = extract_apigw_response_json_body!(response);
        assert_eq!("BAD_QUERY_PARAMETER_VALUE", json["error"]);
        assert_eq!("created", json["source"]["field"]);
        assert_eq!("QUERY", json["source"]["type"]);
    }

    #[tokio::test]
    async fn should_400_when_created_invalid() {
        let mut service = MockItemWatchListService::default();
        service.expect_unwatch().never();

        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::DELETE)
                .path_parameter("shopId", ShopId::new())
                .path_parameter("shopsItemId", ShopsItemId::new())
                .query_string_parameter("created", "boooop")
                .jwt_claim("sub", UserId::new())
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(400, response.status_code);

        let json = extract_apigw_response_json_body!(response);
        assert_eq!("INVALID_RFC3339_TIMESTAMP", json["error"]);
        assert_eq!("created", json["source"]["field"]);
        assert_eq!("QUERY", json["source"]["type"]);
    }

    #[tokio::test]
    async fn should_401_when_sub_missing() {
        let mut service = MockItemWatchListService::default();
        service.expect_unwatch().never();

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
