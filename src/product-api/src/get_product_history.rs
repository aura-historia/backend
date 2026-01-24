use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::currency::data::api::extract_currency_query;
use common::language::data::api::extract_languages_header;
use common::language::domain::Language;
use common::shop_id::api::extract_shop_id_path;
use common::shops_product_id::api::extract_shops_product_id_path;
use lambda_runtime::LambdaEvent;
use product::data::get_product_event_data::GetProductEventData;
use product::service::get_service::GetProductService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    get_product_service: &impl GetProductService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let languages = extract_languages_header(&event.payload.headers)?
        .into_iter()
        .map(Language::from)
        .collect::<Vec<_>>();
    let currency = extract_currency_query(&event.payload.query_string_parameters)?.into();
    let shop_id = extract_shop_id_path(&event.payload.path_parameters)?;
    let shops_product_id = extract_shops_product_id_path(&event.payload.path_parameters)?;

    let product_events_data = get_product_service
        .view_product_history(&shop_id, &shops_product_id, languages.as_slice(), &currency)
        .await?
        .into_iter()
        .map(GetProductEventData::from)
        .collect::<Vec<_>>();

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body_serde(product_events_data)?
        .build())
}

#[cfg(test)]
mod tests {
    use super::handle;
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;
    use lambda_runtime::LambdaEvent;
    use product::service::get_service::{GetProductError, MockGetProductService};
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    async fn should_400_when_path_param_shop_id_is_missing() {
        let mut get_product_service = MockGetProductService::default();
        get_product_service.expect_view_product_history().never();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/products/{shopId}/{shopsProductId}/history".to_owned())
                .path_parameter("shopsProductId", ShopsProductId::new())
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &get_product_service)
            .await
            .unwrap_err();
        assert_eq!(400, actual.status);
    }

    #[tokio::test]
    async fn should_400_when_path_param_shops_product_id_is_missing() {
        let mut get_product_service = MockGetProductService::default();
        get_product_service.expect_view_product_history().never();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/products/{shopId}/{shopsProductId}/history".to_owned())
                .path_parameter("shopId", ShopId::new())
                .build(),
            context: Default::default(),
        };

        let actual = handle(lambda_event, &get_product_service)
            .await
            .unwrap_err();
        assert_eq!(400, actual.status);
    }

    #[tokio::test]
    async fn should_404_when_product_does_not_exist() {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .route_key("GET /api/v1/products/{shopId}/{shopsProductId}/history".to_owned())
                .path_parameter("shopId", shop_id)
                .path_parameter("shopsProductId", shops_product_id)
                .build(),
            context: Default::default(),
        };

        let mut get_product_service = MockGetProductService::default();
        get_product_service
            .expect_view_product_history()
            .return_once(move |shop_id, shops_product_id, _, _| {
                let shop_id = *shop_id;
                let shops_product_id = shops_product_id.clone();
                Box::pin(
                    async move { Err(GetProductError::ProductNotFound(shop_id, shops_product_id)) },
                )
            });

        let actual = handle(lambda_event, &get_product_service)
            .await
            .unwrap_err();
        assert_eq!(404, actual.status);
    }
}
