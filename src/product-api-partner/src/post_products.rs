use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::{BAD_BODY_VALUE, INVALID_JSON, SERVICE_UNAVAILABLE};
use common::shop_id::api::extract_shop_id_path;
use lambda_runtime::LambdaEvent;
use product::data::post_product_data::PostProductData;
use product_lambda_ingest_partner_products::{
    AsyncProductCommandData, AsyncProductCommandService, CreateAsyncProductCommandData,
};
use serde::Serialize;
use shop::core::partner_shop_api_key::api::extract_api_key;
use shop::service::get_service::GetShopService;
use std::collections::HashMap;

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

    let products: Vec<PostProductData> = extract_body(&event.payload)?;

    let commands: Vec<AsyncProductCommandData> = products
        .into_iter()
        .map(|data| {
            AsyncProductCommandData::Create(CreateAsyncProductCommandData::from((
                partner_shop.shop_id,
                data,
            )))
        })
        .collect();

    let failures = async_product_command_service.send(commands.clone()).await;
    if failures.len() == commands.len() && !commands.is_empty() {
        let msg = failures
            .first()
            .map(|failure| failure.error.clone())
            .unwrap_or_else(|| "Failed forwarding product commands to SQS.".to_string());
        return Err(ApiError::service_unavailable(
            SERVICE_UNAVAILABLE,
            msg.into(),
        ));
    }

    let errors: HashMap<String, String> = failures
        .into_iter()
        .map(|failure| {
            (
                failure.command.shops_product_id().to_string(),
                "CREATE_FAILED".to_string(),
            )
        })
        .collect();

    let response = PostProductsResponse { errors };

    Ok(ApiGatewayV2HttpResponseBuilder::json(202)
        .body_serde(response)?
        .build())
}

fn extract_body(request: &ApiGatewayV2httpRequest) -> Result<Vec<PostProductData>, ApiError> {
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
fn to_create_command(
    data: PostProductData,
    partner_shop: &shop::core::partner_shop::PartnerShop,
) -> product::service::product_command::CreateProductCommand {
    CreateAsyncProductCommandData::from((partner_shop.shop_id, data)).into()
}

/// Response for the batch product creation endpoint.
/// Contains a map of `shopsProductId → error key` for products that failed to create.
/// An empty `errors` map indicates all products were created successfully.
#[derive(Debug, Serialize)]
pub struct PostProductsResponse {
    pub errors: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::language::data::{LanguageData, LocalizedTextData};
    use common::shops_product_id::ShopsProductId;
    use fake::{Fake, Faker};
    use http::HeaderMap;
    use lambda_runtime::LambdaEvent;
    use product::data::product_state_data::ProductStateData;
    use product::service::command_service::MockCommandProductService;
    use shop::core::partner_shop::PartnerShop;
    use shop::core::partner_shop_api_key::{HashedPartnerShopApiKey, PartnerShopApiKey};
    use shop::core::shop_type::ShopType;
    use shop::service::get_service::MockGetShopService;

    fn make_event_with_body_and_key(
        shop_id: &common::shop_id::ShopId,
        api_key: &PartnerShopApiKey,
        body: Option<String>,
    ) -> LambdaEvent<ApiGatewayV2httpRequest> {
        let mut request = ApiGatewayV2httpRequest::default();
        request.route_key = Some("POST /api/v1/shops/{shopId}/products".to_string());
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

    fn make_partner_shop_with_type(shop_type: ShopType) -> (PartnerShopApiKey, PartnerShop) {
        let api_key = PartnerShopApiKey::new();
        let mut partner_shop: PartnerShop = Faker.fake();
        partner_shop.shop_type = shop_type;
        let hashed: HashedPartnerShopApiKey = api_key.clone().into();
        partner_shop.hashed_api_key = Some(hashed);
        (api_key, partner_shop)
    }

    #[tokio::test]
    async fn should_return_200_with_empty_errors_when_all_products_created_successfully() {
        let (api_key, partner_shop) = make_partner_shop_with_type(ShopType::AuctionHouse);
        let shop_id = partner_shop.shop_id;

        let body = serde_json::to_string(&vec![serde_json::json!({
            "shopsProductId": "test-product-1",
            "title": { "text": "Test Product", "language": "en" },
            "description": { "text": "A test product", "language": "en" },
            "state": "AVAILABLE",
            "url": "https://example.com/product/1",
            "images": ["https://example.com/img.jpg"]
        })])
        .unwrap();

        let event = make_event_with_body_and_key(&shop_id, &api_key, Some(body));

        let expected_partner = partner_shop.clone();
        let mut shop_service = MockGetShopService::default();
        shop_service
            .expect_verify_partner_shop()
            .return_once(move |_, _| Box::pin(async move { Ok(expected_partner) }));

        let mut command_service = MockCommandProductService::default();
        command_service
            .expect_create()
            .return_once(|_| Box::pin(async { vec![] }));

        let result = handle(event, &shop_service, &command_service).await;
        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status_code, 202);

        let body: serde_json::Value = match response.body {
            Some(aws_lambda_events::encodings::Body::Text(body_str)) => {
                serde_json::from_str(&body_str).unwrap()
            }
            _ => panic!("Expected response body to be Text"),
        };
        assert!(body["errors"].as_object().unwrap().is_empty());
    }

    #[tokio::test]
    async fn should_return_200_with_error_entries_when_some_products_fail() {
        let (api_key, partner_shop) = make_partner_shop_with_type(ShopType::AuctionHouse);
        let shop_id = partner_shop.shop_id;
        let shops_product_id = ShopsProductId::from("failing-product".to_string());

        let body = serde_json::to_string(&vec![serde_json::json!({
            "shopsProductId": "failing-product",
            "title": { "text": "Test Product", "language": "en" },
            "description": { "text": "A test product", "language": "en" },
            "state": "AVAILABLE",
            "url": "https://example.com/product/1",
            "images": []
        })])
        .unwrap();

        let event = make_event_with_body_and_key(&shop_id, &api_key, Some(body));

        let expected_partner = partner_shop.clone();
        let mut shop_service = MockGetShopService::default();
        shop_service
            .expect_verify_partner_shop()
            .return_once(move |_, _| Box::pin(async move { Ok(expected_partner) }));

        let expected_cmd = to_create_command(
            PostProductData {
                shops_product_id: shops_product_id.clone(),
                title: LocalizedTextData::new("Test Product", LanguageData::En),
                description: LocalizedTextData::new("A test product", LanguageData::En),
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
                state: ProductStateData::Available,
                url: url::Url::parse("https://example.com/product/1").unwrap(),
                images: vec![],
                auction_start: None,
                auction_end: None,
                seller_name: None,
                structured_address: None,
                geo_address: None,
            },
            &partner_shop,
        );

        let mut command_service = MockCommandProductService::default();
        command_service
            .expect_create()
            .return_once(move |_| Box::pin(async move { vec![expected_cmd] }));

        let err = handle(event, &shop_service, &command_service)
            .await
            .unwrap_err();
        assert_eq!(err.status, 503);
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
        let command_service = MockCommandProductService::default();
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
        let command_service = MockCommandProductService::default();
        let result = handle(event, &shop_service, &command_service).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().status, 400);
    }

    #[tokio::test]
    async fn should_pass_seller_name_raw_to_command_when_seller_name_provided() {
        let (api_key, partner_shop) = make_partner_shop_with_type(ShopType::Marketplace);
        let shop_id = partner_shop.shop_id;

        let body = serde_json::to_string(&vec![serde_json::json!({
            "shopsProductId": "test-product-2",
            "title": { "text": "Marketplace Product", "language": "en" },
            "description": { "text": "A marketplace product", "language": "en" },
            "state": "LISTED",
            "url": "https://example.com/product/2",
            "images": [],
            "sellerName": "marketplace seller raw"
        })])
        .unwrap();

        let event = make_event_with_body_and_key(&shop_id, &api_key, Some(body));

        let expected_partner = partner_shop.clone();
        let mut shop_service = MockGetShopService::default();
        shop_service
            .expect_verify_partner_shop()
            .return_once(move |_, _| Box::pin(async move { Ok(expected_partner) }));

        let mut command_service = MockCommandProductService::default();
        command_service.expect_create().return_once(move |cmds| {
            Box::pin(async move {
                assert_eq!(
                    cmds[0].seller_name_raw.as_deref(),
                    Some("marketplace seller raw")
                );
                vec![]
            })
        });

        let result = handle(event, &shop_service, &command_service).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().status_code, 202);
    }

    #[test]
    fn should_convert_post_product_data_to_create_command_for_mapping() {
        let partner_shop: PartnerShop = Faker.fake();
        let data = PostProductData {
            shops_product_id: ShopsProductId::from("test-id".to_string()),
            title: LocalizedTextData::new("Test Title", LanguageData::De),
            description: LocalizedTextData::new("Test Description", LanguageData::De),
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: ProductStateData::Listed,
            url: url::Url::parse("https://example.com").unwrap(),
            images: vec![url::Url::parse("https://example.com/img.jpg").unwrap()],
            auction_start: None,
            auction_end: None,
            seller_name: Some("Test Seller".to_string()),
            structured_address: None,
            geo_address: None,
        };

        let cmd = to_create_command(data, &partner_shop);

        assert_eq!(cmd.shop_id, partner_shop.shop_id);
        assert_eq!(
            cmd.shops_product_id,
            ShopsProductId::from("test-id".to_string())
        );
        assert_eq!(cmd.images.len(), 1);
        assert!(cmd.native_price.is_none());
        assert_eq!(cmd.seller_name_raw.as_deref(), Some("Test Seller"));
    }
}
