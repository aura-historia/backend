use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::{BAD_BODY_VALUE, INVALID_JSON, SERVICE_UNAVAILABLE};
use common::has_key::HasKey;
use common::shop_id::api::extract_shop_id_path;
use lambda_runtime::LambdaEvent;
use product::data::patch_product_data::PatchProductData;
use product_lambda_ingest_partner_products::{
    AsyncProductCommandData, AsyncProductCommandService, UpdateAsyncProductCommandData,
};
use shop::core::partner_shop_api_key::api::extract_api_key;
use shop::service::get_service::GetShopService;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    get_shop_service: &(impl GetShopService + Sync),
    async_product_command_service: &(impl AsyncProductCommandService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let shop_id = extract_shop_id_path(&event.payload.path_parameters)?;
    let api_key = extract_api_key(&event.payload)?;

    let partner_shop = get_shop_service
        .verify_partner_shop(&api_key, &shop_id)
        .await?;

    let products: Vec<PatchProductData> = extract_body(&event.payload)?;

    let commands: Vec<AsyncProductCommandData> = products
        .into_iter()
        .map(|data| {
            AsyncProductCommandData::Update(UpdateAsyncProductCommandData::from((
                partner_shop.shop_id,
                data,
            )))
        })
        .collect();

    let command_count = commands.len();
    let failures = async_product_command_service.send(commands).await;
    if failures.len() == command_count && command_count > 0 {
        let msg = failures
            .first()
            .map(|failure| failure.error.clone())
            .unwrap_or_else(|| "Failed forwarding product commands to SQS.".to_string());
        return Err(ApiError::service_unavailable(
            SERVICE_UNAVAILABLE,
            msg.into(),
        ));
    }

    let failed_shops_product_ids: Vec<String> = failures
        .into_iter()
        .map(|failure| failure.command.key().shops_product_id.to_string())
        .collect();

    Ok(ApiGatewayV2HttpResponseBuilder::json(202)
        .body_serde(failed_shops_product_ids)?
        .build())
}

fn extract_body(request: &ApiGatewayV2httpRequest) -> Result<Vec<PatchProductData>, ApiError> {
    let body = request
        .body
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request(BAD_BODY_VALUE, "Body cannot be empty.".into())
                .with_detail("Body cannot be empty.")
        })?;

    serde_json::from_str(body).map_err(|err| {
        let msg = err.to_string();
        ApiError::bad_request(INVALID_JSON, Box::new(err)).with_detail(msg)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::shops_product_id::ShopsProductId;
    use fake::{Fake, Faker};
    use http::HeaderMap;
    use lambda_runtime::LambdaEvent;
    use product::data::product_state_data::ProductStateData;
    use product_lambda_ingest_partner_products::service::{
        AsyncProductCommandFailure, MockAsyncProductCommandService,
    };
    use shop::core::partner_shop::PartnerShop;
    use shop::core::partner_shop_api_key::{HashedPartnerShopApiKey, PartnerShopApiKey};
    use shop::service::get_service::MockGetShopService;

    fn make_event_with_body_and_key(
        shop_id: &common::shop_id::ShopId,
        api_key: &PartnerShopApiKey,
        body: Option<String>,
    ) -> LambdaEvent<ApiGatewayV2httpRequest> {
        let mut request = ApiGatewayV2httpRequest::default();
        request.route_key = Some("PATCH /api/v1/shops/{shopId}/products".to_string());
        request
            .path_parameters
            .insert("shopId".to_string(), shop_id.to_string());
        let key_str: String = api_key.clone().into();
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", key_str.parse().unwrap());
        request.headers = headers;
        request.body = body;
        LambdaEvent::new(request, lambda_runtime::Context::default())
    }

    #[tokio::test]
    async fn should_return_202_with_empty_failures_when_all_products_forwarded_successfully() {
        let api_key = PartnerShopApiKey::new();
        let partner_shop: PartnerShop = Faker.fake();
        let shop_id = partner_shop.shop_id;
        let hashed: HashedPartnerShopApiKey = api_key.clone().into();
        let mut partner_shop_with_key = partner_shop;
        partner_shop_with_key.hashed_api_key = Some(hashed);

        let body = serde_json::to_string(&vec![serde_json::json!({
            "shopsProductId": "test-product-1",
            "state": "AVAILABLE"
        })])
        .unwrap();

        let event = make_event_with_body_and_key(&shop_id, &api_key, Some(body));

        let expected_partner = partner_shop_with_key.clone();
        let mut shop_service = MockGetShopService::default();
        shop_service
            .expect_verify_partner_shop()
            .return_once(move |_, _| Box::pin(async move { Ok(expected_partner) }));

        let mut command_service = MockAsyncProductCommandService::default();
        command_service
            .expect_send()
            .return_once(|_| Box::pin(async { vec![] }));

        let result = handle(event, &shop_service, &command_service).await;
        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status_code, 202);

        let body: Vec<String> = match response.body {
            Some(aws_lambda_events::encodings::Body::Text(body_str)) => {
                serde_json::from_str(&body_str).unwrap()
            }
            _ => panic!("Expected response body to be Text"),
        };
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn should_return_202_with_failed_product_ids_when_some_products_fail_to_forward() {
        let api_key = PartnerShopApiKey::new();
        let partner_shop: PartnerShop = Faker.fake();
        let shop_id = partner_shop.shop_id;
        let hashed: HashedPartnerShopApiKey = api_key.clone().into();
        let mut partner_shop_with_key = partner_shop;
        partner_shop_with_key.hashed_api_key = Some(hashed);

        let shops_product_id = ShopsProductId::from("failing-product".to_string());

        let body = serde_json::to_string(&vec![
            serde_json::json!({
                "shopsProductId": "successful-product",
                "state": "AVAILABLE"
            }),
            serde_json::json!({
                "shopsProductId": "failing-product",
                "state": "AVAILABLE"
            }),
        ])
        .unwrap();

        let event = make_event_with_body_and_key(&shop_id, &api_key, Some(body));

        let expected_partner = partner_shop_with_key.clone();
        let mut shop_service = MockGetShopService::default();
        shop_service
            .expect_verify_partner_shop()
            .return_once(move |_, _| Box::pin(async move { Ok(expected_partner) }));

        let failed_command =
            AsyncProductCommandData::Update(UpdateAsyncProductCommandData::from((
                partner_shop_with_key.shop_id,
                PatchProductData {
                    shops_product_id: shops_product_id.clone(),
                    price: None,
                    state: Some(ProductStateData::Available),
                    price_estimate_min: None,
                    price_estimate_max: None,
                    url: None,
                    images: None,
                    auction_start: None,
                    auction_end: None,
                },
            )));

        let mut command_service = MockAsyncProductCommandService::default();
        command_service.expect_send().return_once(move |_| {
            Box::pin(async move {
                vec![AsyncProductCommandFailure {
                    command: failed_command,
                    error: "failed".to_string(),
                }]
            })
        });

        let response = handle(event, &shop_service, &command_service)
            .await
            .unwrap();
        assert_eq!(response.status_code, 202);
        let body: Vec<String> = match response.body {
            Some(aws_lambda_events::encodings::Body::Text(body_str)) => {
                serde_json::from_str(&body_str).unwrap()
            }
            _ => panic!("Expected response body to be Text"),
        };
        assert_eq!(body, vec!["failing-product"]);
    }

    #[tokio::test]
    async fn should_return_400_when_body_is_empty() {
        let api_key = PartnerShopApiKey::new();
        let shop_id = common::shop_id::ShopId::new();

        let event = make_event_with_body_and_key(&shop_id, &api_key, None);

        let mut shop_service = MockGetShopService::default();
        shop_service
            .expect_verify_partner_shop()
            .return_once(move |_, _| {
                let partner: PartnerShop = Faker.fake();
                Box::pin(async move { Ok(partner) })
            });
        let command_service = MockAsyncProductCommandService::default();

        let result = handle(event, &shop_service, &command_service).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().status, 400);
    }

    #[tokio::test]
    async fn should_return_400_when_body_is_invalid_json() {
        let api_key = PartnerShopApiKey::new();
        let shop_id = common::shop_id::ShopId::new();

        let event = make_event_with_body_and_key(&shop_id, &api_key, Some("not json".to_string()));

        let mut shop_service = MockGetShopService::default();
        shop_service
            .expect_verify_partner_shop()
            .return_once(move |_, _| {
                let partner: PartnerShop = Faker.fake();
                Box::pin(async move { Ok(partner) })
            });
        let command_service = MockAsyncProductCommandService::default();

        let result = handle(event, &shop_service, &command_service).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().status, 400);
    }

    #[test]
    fn should_convert_patch_product_data_to_update_entry_for_mapping() {
        let partner_shop: PartnerShop = Faker.fake();
        let data = PatchProductData {
            shops_product_id: ShopsProductId::from("test-id".to_string()),
            price: None,
            state: Some(ProductStateData::Listed),
            price_estimate_min: None,
            price_estimate_max: None,
            url: None,
            images: None,
            auction_start: None,
            auction_end: None,
        };

        let (key, cmd): (
            common::product_id::ProductKey,
            product::service::product_command::UpdateProductCommand,
        ) = UpdateAsyncProductCommandData::from((partner_shop.shop_id, data)).into();

        assert_eq!(key.shop_id, partner_shop.shop_id);
        assert_eq!(
            key.shops_product_id,
            ShopsProductId::from("test-id".to_string())
        );
        assert!(cmd.native_price.is_none());
        assert_eq!(
            cmd.state,
            Some(common::product_state::domain::ProductState::Listed)
        );
    }

    #[test]
    fn should_convert_patch_product_data_with_price_to_update_entry_for_mapping() {
        let partner_shop: PartnerShop = Faker.fake();
        let data = PatchProductData {
            shops_product_id: ShopsProductId::from("test-id".to_string()),
            price: Some(common::price::data::PriceData::new(
                common::currency::data::CurrencyData::Eur,
                1000,
            )),
            state: Some(ProductStateData::Available),
            price_estimate_min: None,
            price_estimate_max: None,
            url: None,
            images: None,
            auction_start: None,
            auction_end: None,
        };

        let (key, cmd): (
            common::product_id::ProductKey,
            product::service::product_command::UpdateProductCommand,
        ) = UpdateAsyncProductCommandData::from((partner_shop.shop_id, data)).into();

        assert_eq!(key.shop_id, partner_shop.shop_id);
        assert!(cmd.native_price.is_some());
        assert_eq!(
            cmd.state,
            Some(common::product_state::domain::ProductState::Available)
        );
    }
}
