use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::BAD_BODY_VALUE;
use lambda_runtime::LambdaEvent;
use shop::data::get_shop_data::GetShopData;
use shop::data::post_shop_data::PostShopData;
use shop::service::command::CreateShopCommand;
use shop::service::command_service::CommandShopService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    service: &impl CommandShopService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let body = event
        .payload
        .body
        .filter(|str| !str.is_empty())
        .ok_or_else(|| {
            let err_msg = "Body cannot be empty";
            ApiError::bad_request(BAD_BODY_VALUE, err_msg.into()).with_detail(err_msg)
        })?;
    let post_shop_data: PostShopData = serde_json::from_str(&body).map_err(|err| {
        let err_msg = err.to_string();
        ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(err_msg)
    })?;

    let create_shop_command = CreateShopCommand {
        name: post_shop_data.name,
        shop_type: post_shop_data.shop_type.into(),
        domains: post_shop_data.domains,
        image: post_shop_data.image,
    };
    let created_shop = service.create(create_shop_command).await?;

    let location = match event.payload.request_context.domain_name {
        None => None,
        Some(domain_name) => match event.payload.request_context.stage {
            Some(stage_name) => Some(format!(
                "https://{domain_name}/{stage_name}/api/v1/shops/{}",
                created_shop.shop_id
            )),
            None => None,
        },
    };
    let created_shop_data = GetShopData::from(created_shop);

    Ok(ApiGatewayV2HttpResponseBuilder::json(201)
        .try_location(location.as_deref())
        .last_modified(created_shop_data.updated)
        .body_serde(created_shop_data)?
        .build())
}

#[cfg(test)]
mod tests {
    use crate::handle;
    use fake::{Fake, Faker};
    use http::header::LAST_MODIFIED;
    use lambda_runtime::LambdaEvent;
    use serde_json::json;
    use shop::core::shop::Shop;
    use shop::data::post_shop_data::PostShopData;
    use shop::service::command_service::MockCommandShopService;
    use shop::service::get_service::MockGetShopService;
    use shop::service::query_service::MockQueryShopService;
    use test_api::ApiGatewayV2httpRequestProxy;
    use time::macros::datetime;

    #[tokio::test]
    async fn should_201_when_success() {
        let mut service = MockCommandShopService::default();
        service.expect_create().return_once(move |_| {
            let shop: Shop = Faker.fake();
            Box::pin(async move { Ok(shop) })
        });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/shops")
                .body_serde(&Faker.fake::<PostShopData>())
                .build(),
            context: Default::default(),
        };
        let response = handle(
            lambda_event,
            &MockGetShopService::default(),
            &MockQueryShopService::default(),
            &service,
        )
        .await
        .unwrap();
        assert_eq!(201, response.status_code);
    }

    #[tokio::test]
    async fn should_include_updated_timestamp_as_header_last_modified() {
        let timestamp = datetime!(2020-01-01 0:00 UTC);
        let mut service = MockCommandShopService::default();
        service.expect_create().return_once(move |_| {
            let mut shop: Shop = Faker.fake();
            shop.updated = timestamp;
            Box::pin(async move { Ok(shop) })
        });
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/shops")
                .body_serde(&Faker.fake::<PostShopData>())
                .build(),
            context: Default::default(),
        };
        let response = handle(
            lambda_event,
            &MockGetShopService::default(),
            &MockQueryShopService::default(),
            &service,
        )
        .await
        .unwrap();
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
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/shops")
                .build(),
            context: Default::default(),
        };
        let response = handle(
            lambda_event,
            &MockGetShopService::default(),
            &MockQueryShopService::default(),
            &service,
        )
        .await
        .unwrap_err();
        assert_eq!(400, response.status);
    }

    #[tokio::test]
    async fn should_400_when_body_wrong_payload() {
        let service = MockCommandShopService::default();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::POST)
                .route_key("POST /api/v1/shops")
                .body_serde(&json!({
                    "foo": 42
                }))
                .build(),
            context: Default::default(),
        };
        let response = handle(
            lambda_event,
            &MockGetShopService::default(),
            &MockQueryShopService::default(),
            &service,
        )
        .await
        .unwrap_err();
        assert_eq!(400, response.status);
    }
}
