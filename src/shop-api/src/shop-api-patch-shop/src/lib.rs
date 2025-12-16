use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::{ApiError, log_api_error};
use common::api::error_code::BAD_BODY_VALUE;
use lambda_runtime::LambdaEvent;
use shop::data::get_shop_data::GetShopData;
use shop::data::patch_shop_data::PatchShopData;
use shop::data::shop_identifier_data::extract_shop_identifier_data_path;
use shop::service::command::UpdateShopCommand;
use shop::service::command_service::CommandShopService;

#[tracing::instrument(
    skip(event, service),
    fields(
        requestId = %event.context.request_id,
        method = event.payload.request_context.http.method.as_str(),
        path = &event.payload.raw_path.as_deref().unwrap_or("NULL"),
        query = &event.payload.raw_query_string.as_deref().unwrap_or("NULL"),
        body = &event.payload.body.as_deref().unwrap_or("NULL"),
        ip = &event.payload.request_context.http.source_ip.as_deref().unwrap_or("NULL"),
        userAgent = &event.payload.request_context.http.user_agent.as_deref().unwrap_or("NULL"),
    )
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl CommandShopService,
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, service).await {
        Ok(response) => Ok(response),
        Err(err) => {
            log_api_error(&err);
            Ok(ApiGatewayV2httpResponse::from(err))
        }
    }
}

// PATCH /api/v1/shops/{shopIdentifier}
pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl CommandShopService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let shop_identifier = extract_shop_identifier_data_path(&event.payload.path_parameters)?.into();
    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            let err_msg = "Body cannot be empty";
            ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_detail(err_msg)
        })?;
    let patch_shop_data: PatchShopData = serde_json::from_str(&body).map_err(|err| {
        let err_msg = err.to_string();
        ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
    })?;

    let update_shop_command = UpdateShopCommand {
        name: patch_shop_data.name,
        domains: patch_shop_data.domains,
        image: patch_shop_data.image,
    };
    let updated_shop = service
        .update(&shop_identifier, update_shop_command)
        .await?;
    let updated_shop_data = GetShopData::from(updated_shop);

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .last_modified(updated_shop_data.updated)
        .body_serde(updated_shop_data)?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handler;
    use common::shop_id::ShopId;
    use fake::{Fake, Faker};
    use http::header::LAST_MODIFIED;
    use lambda_runtime::LambdaEvent;
    use serde_json::json;
    use shop::core::shop::Shop;
    use shop::data::patch_shop_data::PatchShopData;
    use shop::service::command_service::MockCommandShopService;
    use test_api::ApiGatewayV2httpRequestProxy;
    use time::macros::datetime;

    #[tokio::test]
    async fn should_200_when_success_for_shop_id() {
        let mut service = MockCommandShopService::default();
        service.expect_update().return_once(move |_, _| {
            let shop: Shop = Faker.fake();
            Box::pin(async move { Ok(shop) })
        });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .path_parameter("shopIdentifier", ShopId::new())
                .http_method(http::Method::PATCH)
                .body_serde(&Faker.fake::<PatchShopData>())
                .build(),
            context: Default::default(),
        };
        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_200_when_success_for_shop_domain() {
        let mut service = MockCommandShopService::default();
        service.expect_update().return_once(move |_, _| {
            let shop: Shop = Faker.fake();
            Box::pin(async move { Ok(shop) })
        });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .path_parameter("shopIdentifier", "boop.com")
                .http_method(http::Method::PATCH)
                .body_serde(&Faker.fake::<PatchShopData>())
                .build(),
            context: Default::default(),
        };
        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_include_updated_timestamp_as_header_last_modified() {
        let timestamp = datetime!(2020-01-01 0:00 UTC);
        let mut service = MockCommandShopService::default();
        service.expect_update().return_once(move |_, _| {
            let mut shop: Shop = Faker.fake();
            shop.updated = timestamp;
            Box::pin(async move { Ok(shop) })
        });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .path_parameter("shopIdentifier", ShopId::new())
                .http_method(http::Method::PATCH)
                .body_serde(&Faker.fake::<PatchShopData>())
                .build(),
            context: Default::default(),
        };
        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(
            "Wed, 01 Jan 2020 00:00:00 GMT",
            response.headers.get(LAST_MODIFIED).unwrap()
        );
    }

    #[tokio::test]
    async fn should_400_when_body_empty() {
        let service = MockCommandShopService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .build(),
            context: Default::default(),
        };
        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(400, response.status_code);
    }

    #[tokio::test]
    async fn should_400_when_body_wrong_payload() {
        let service = MockCommandShopService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .body_serde(&json!({
                    "foo": 42
                }))
                .build(),
            context: Default::default(),
        };
        let response = handler(lambda_event, &service).await.unwrap();
        assert_eq!(400, response.status_code);
    }
}
