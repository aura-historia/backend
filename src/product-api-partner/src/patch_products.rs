use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::{BAD_BODY_VALUE, INVALID_JSON};
use common::price::domain::Price;
use common::product_id::ProductKey;
use common::shop_id::api::extract_shop_id_path;
use lambda_runtime::LambdaEvent;
use product::data::patch_product_data::PatchProductData;
use product::service::command_service::CommandProductService;
use product::service::product_command::UpdateProductCommand;
use serde::Serialize;
use shop::core::partner_shop::PartnerShop;
use shop::core::partner_shop_api_key::api::extract_api_key;
use shop::service::get_service::GetShopService;
use std::collections::HashMap;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    get_shop_service: &(impl GetShopService + Sync),
    command_product_service: &(impl CommandProductService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let shop_id = extract_shop_id_path(&event.payload.path_parameters)?;
    let api_key = extract_api_key(&event.payload)?;

    let partner_shop = get_shop_service
        .verify_partner_shop(&api_key, &shop_id)
        .await?;

    let products: Vec<PatchProductData> = extract_body(&event.payload)?;

    let commands: HashMap<ProductKey, UpdateProductCommand> = products
        .into_iter()
        .map(|data| to_update_entry(data, &partner_shop))
        .collect();

    let failures = command_product_service.update(commands).await;

    let errors: HashMap<String, String> = failures
        .into_keys()
        .map(|key| {
            (
                key.shops_product_id.to_string(),
                "UPDATE_FAILED".to_string(),
            )
        })
        .collect();

    let response = PatchProductsResponse { errors };

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body_serde(response)?
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

fn to_update_entry(
    data: PatchProductData,
    partner_shop: &PartnerShop,
) -> (ProductKey, UpdateProductCommand) {
    let key = ProductKey::new(partner_shop.shop_id, data.shops_product_id);

    let images = data.images.map(|urls| {
        urls.into_iter()
            .map(|url| product::core::product_image::ProductImage {
                url,
                prohibited_content: product::core::prohibited_content::ProhibitedContent::default(),
            })
            .collect()
    });

    let cmd = UpdateProductCommand {
        native_price: data.price.map(Price::from),
        state: data.state.map(|s| s.into()),
        native_price_estimate_min: data.price_estimate_min.map(Price::from),
        native_price_estimate_max: data.price_estimate_max.map(Price::from),
        url: data.url,
        images,
        auction_start: data.auction_start,
        auction_end: data.auction_end,
        origin_year: data.origin_year.map(|oy| oy.into()),
        authenticity: data.authenticity.map(|a| a.into()),
        condition: data.condition.map(|c| c.into()),
        provenance: data.provenance.map(|p| p.into()),
        restoration: data.restoration.map(|r| r.into()),
    };
    (key, cmd)
}

/// Response for the batch product update endpoint.
/// Contains a map of `shopsProductId → error key` for products that failed to update.
/// An empty `errors` map indicates all products were updated successfully.
#[derive(Debug, Serialize)]
pub struct PatchProductsResponse {
    pub errors: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::shops_product_id::ShopsProductId;
    use fake::{Fake, Faker};
    use http::HeaderMap;
    use lambda_runtime::LambdaEvent;
    use product::data::product_state_data::ProductStateData;
    use product::service::command_service::MockCommandProductService;
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
    async fn should_return_200_with_empty_errors_when_all_products_updated_successfully() {
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

        let mut command_service = MockCommandProductService::default();
        command_service
            .expect_update()
            .return_once(|_| Box::pin(async { HashMap::new() }));

        let result = handle(event, &shop_service, &command_service).await;
        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status_code, 200);

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
        let api_key = PartnerShopApiKey::new();
        let partner_shop: PartnerShop = Faker.fake();
        let shop_id = partner_shop.shop_id;
        let hashed: HashedPartnerShopApiKey = api_key.clone().into();
        let mut partner_shop_with_key = partner_shop;
        partner_shop_with_key.hashed_api_key = Some(hashed);

        let shops_product_id = ShopsProductId::from("failing-product".to_string());

        let body = serde_json::to_string(&vec![serde_json::json!({
            "shopsProductId": "failing-product",
            "state": "AVAILABLE"
        })])
        .unwrap();

        let event = make_event_with_body_and_key(&shop_id, &api_key, Some(body));

        let expected_partner = partner_shop_with_key.clone();
        let mut shop_service = MockGetShopService::default();
        shop_service
            .expect_verify_partner_shop()
            .return_once(move |_, _| Box::pin(async move { Ok(expected_partner) }));

        let (key, cmd) = to_update_entry(
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
                origin_year: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
            },
            &partner_shop_with_key,
        );

        let mut command_service = MockCommandProductService::default();
        command_service
            .expect_update()
            .return_once(move |_| Box::pin(async move { HashMap::from([(key, cmd)]) }));

        let result = handle(event, &shop_service, &command_service).await;
        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status_code, 200);

        let body: serde_json::Value = match response.body {
            Some(aws_lambda_events::encodings::Body::Text(body_str)) => {
                serde_json::from_str(&body_str).unwrap()
            }
            _ => panic!("Expected response body to be Text"),
        };
        assert!(
            body["errors"]
                .as_object()
                .unwrap()
                .contains_key("failing-product")
        );
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
            origin_year: None,
            authenticity: None,
            condition: None,
            provenance: None,
            restoration: None,
        };

        let (key, cmd) = to_update_entry(data, &partner_shop);

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
            origin_year: None,
            authenticity: None,
            condition: None,
            provenance: None,
            restoration: None,
        };

        let (key, cmd) = to_update_entry(data, &partner_shop);

        assert_eq!(key.shop_id, partner_shop.shop_id);
        assert!(cmd.native_price.is_some());
        assert_eq!(
            cmd.state,
            Some(common::product_state::domain::ProductState::Available)
        );
    }
}
