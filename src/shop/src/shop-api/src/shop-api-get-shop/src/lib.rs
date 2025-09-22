use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::{BAD_PATH_PARAMETER_VALUE, INVALID_UUID};
use common::shop_id::ShopId;
use lambda_runtime::LambdaEvent;
use shop_data::get_shop_data::GetShopData;
use shop_service::get_service::GetShopService;

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
    service: &impl GetShopService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, service).await {
        Ok(response) => Ok(response),
        Err(err) => Ok(ApiGatewayV2httpResponse::from(err)),
    }
}

// GET /api/v1/shops/{shopId}
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl GetShopService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let shop_id = event
        .payload
        .path_parameters
        .get("shopId")
        .map(ShopId::try_from)
        .transpose()
        .map_err(|err| {
            ApiError::bad_request(INVALID_UUID)
                .with_path_field("shopId")
                .with_message(err.to_string())
        })?
        .ok_or(ApiError::bad_request(BAD_PATH_PARAMETER_VALUE).with_path_field("shopId"))?;

    let shop_data: GetShopData = service.find_shop(&shop_id).await?.into();

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .last_modified(shop_data.updated)
        .body_serde(shop_data)?
        .cors()
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handler;
    use common::shop_id::ShopId;
    use fake::{Fake, Faker};
    use http::header::LAST_MODIFIED;
    use lambda_runtime::LambdaEvent;
    use shop_core::shop::Shop;
    use shop_service::get_service::{GetShopError, MockGetShopService};
    use test_api::{ApiGatewayV2httpRequestProxy, extract_apigw_response_json_body};
    use time::macros::datetime;

    #[tokio::test]
    async fn should_include_updated_timestamp_as_header_last_modified() {
        let timestamp = datetime!(2020-01-01 0:00 UTC);
        let mut service = MockGetShopService::default();
        service.expect_find_shop().return_once(move |_| {
            let mut shop: Shop = Faker.fake();
            shop.updated = timestamp;
            Box::pin(async move { Ok(shop) })
        });
        let shop_id = ShopId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopId", shop_id)
                .build(),
            context: Default::default(),
        };
        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
        assert_eq!(
            "Wed, 01 Jan 2020 00:00:00 GMT",
            response.headers.get(LAST_MODIFIED).unwrap()
        );
    }

    #[tokio::test]
    async fn should_400_when_path_param_shop_id_is_missing() {
        let mut service = MockGetShopService::default();
        service.expect_find_shop().never();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .build(),
            context: Default::default(),
        };

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(400, response.status_code);
        let json = extract_apigw_response_json_body!(response);
        assert_eq!(400, json["status"]);
        assert_eq!("shopId", json["source"]["field"]);
    }

    #[tokio::test]
    async fn should_404_when_shop_does_not_exist() {
        let shop_id = ShopId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .path_parameter("shopId", shop_id)
                .build(),
            context: Default::default(),
        };

        let mut service = MockGetShopService::default();
        service.expect_find_shop().return_once(move |shop_id| {
            let shop_id = *shop_id;
            Box::pin(async move { Err(GetShopError::ShopNotFound(shop_id)) })
        });

        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(404, response.status_code);
        let json = extract_apigw_response_json_body!(response);
        assert_eq!(404, json["status"]);
    }
}
