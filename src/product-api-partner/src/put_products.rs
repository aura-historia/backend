use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::{BAD_BODY_VALUE, INTERNAL_SERVER_ERROR, INVALID_JSON};
use common::localized::Localized;
use common::price::domain::Price;
use common::shop_id::ShopId;
use common::shop_id::api::extract_shop_id_path;
use common::shop_name::ShopName;
use lambda_runtime::LambdaEvent;
use product::core::product_image::ProductImage;
use product::core::prohibited_content::ProhibitedContent;
use product::data::put_product_data::PutProductData;
use product::service::command_service::CommandProductService;
use product::service::product_command::UpsertProductCommand;
use serde::Serialize;
use shop::core::partner_shop::PartnerShop;
use shop::core::partner_shop_api_key::api::extract_api_key;
use shop::core::shop_type::ShopType;
use shop::service::get_service::GetShopService;
use shop::service::seller_service::{SellerService, SellerServiceError};
use std::collections::HashMap;

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    get_shop_service: &(impl GetShopService + Sync),
    command_product_service: &(impl CommandProductService + Sync),
    seller_service: &dyn SellerService,
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let shop_id = extract_shop_id_path(&event.payload.path_parameters)?;
    let api_key = extract_api_key(&event.payload)?;

    let partner_shop = get_shop_service
        .verify_partner_shop(&api_key, &shop_id)
        .await?;

    let products: Vec<PutProductData> = extract_body(&event.payload)?;

    let mut commands = Vec::with_capacity(products.len());
    for data in products {
        let (seller_id, seller_name) =
            resolve_seller(&partner_shop, &data, seller_service)
                .await
                .map_err(|e| ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(e)))?;
        commands.push(to_upsert_command(
            data,
            &partner_shop,
            seller_id,
            seller_name,
        ));
    }

    let failures = command_product_service.upsert(commands).await;

    let errors: HashMap<String, String> = failures
        .into_iter()
        .map(|cmd| {
            (
                cmd.shops_product_id.to_string(),
                "UPSERT_FAILED".to_string(),
            )
        })
        .collect();

    let response = PutProductsResponse { errors };

    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body_serde(response)?
        .build())
}

fn extract_body(request: &ApiGatewayV2httpRequest) -> Result<Vec<PutProductData>, ApiError> {
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

async fn resolve_seller(
    partner_shop: &PartnerShop,
    data: &PutProductData,
    seller_service: &dyn SellerService,
) -> Result<(ShopId, ShopName), SellerServiceError> {
    match partner_shop.shop_type {
        ShopType::AuctionPlatform | ShopType::Marketplace => {
            if let Some(ref raw_name) = data.seller_name {
                let shop_name = ShopName::from(raw_name.as_str());
                let (id, _, name) = seller_service.get_seller_shop_details(&shop_name).await?;
                return Ok((id, name));
            }
            Ok((partner_shop.shop_id, partner_shop.name.clone()))
        }
        _ => Ok((partner_shop.shop_id, partner_shop.name.clone())),
    }
}

fn to_upsert_command(
    data: PutProductData,
    partner_shop: &PartnerShop,
    seller_id: ShopId,
    seller_name: ShopName,
) -> UpsertProductCommand {
    let native_title: Option<Localized<_, _>> = data.title.map(Into::into);
    let native_description: Option<Localized<_, _>> = data.description.map(Into::into);

    let native_price: Option<Price> = data.price.map(Price::from);
    let native_price_estimate_min: Option<Price> = data.price_estimate_min.map(Price::from);
    let native_price_estimate_max: Option<Price> = data.price_estimate_max.map(Price::from);

    let images: Vec<ProductImage> = data
        .images
        .unwrap_or_default()
        .into_iter()
        .map(|url| ProductImage {
            url,
            prohibited_content: ProhibitedContent::default(),
        })
        .collect();

    let origin_year = data.origin_year.map(|oy| oy.into());

    UpsertProductCommand {
        shop_id: partner_shop.shop_id,
        shops_product_id: data.shops_product_id,
        shop_name: partner_shop.name.clone(),
        shop_type: partner_shop.shop_type,
        native_title,
        native_description,
        native_price,
        native_price_estimate_min,
        native_price_estimate_max,
        state: data.state.map(|s| s.into()),
        url: data.url,
        images,
        auction_start: data.auction_start,
        auction_end: data.auction_end,
        origin_year,
        authenticity: data.authenticity.into(),
        condition: data.condition.into(),
        provenance: data.provenance.into(),
        restoration: data.restoration.into(),
        seller_id,
        seller_name,
    }
}

/// Response for the batch product upsert endpoint.
/// Contains a map of `shopsProductId → error key` for products that failed to upsert.
/// An empty `errors` map indicates all products were upserted successfully.
#[derive(Debug, Serialize)]
pub struct PutProductsResponse {
    pub errors: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;
    use fake::{Fake, Faker};
    use http::HeaderMap;
    use lambda_runtime::LambdaEvent;
    use product::service::command_service::MockCommandProductService;
    use shop::core::partner_shop::PartnerShop;
    use shop::core::partner_shop_api_key::{HashedPartnerShopApiKey, PartnerShopApiKey};
    use shop::service::get_service::MockGetShopService;
    use shop::service::seller_service::MockSellerService;

    fn make_event_with_body_and_key(
        shop_id: &common::shop_id::ShopId,
        api_key: &PartnerShopApiKey,
        body: Option<String>,
    ) -> LambdaEvent<ApiGatewayV2httpRequest> {
        let mut request = ApiGatewayV2httpRequest::default();
        request.route_key = Some("PUT /api/v1/shops/{shopId}/products".to_string());
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
    async fn should_return_200_with_empty_errors_when_all_products_upserted_successfully() {
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
            .expect_upsert()
            .return_once(|_| Box::pin(async { vec![] }));

        let seller_service = MockSellerService::default();

        let result = handle(event, &shop_service, &command_service, &seller_service).await;
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
        let (api_key, partner_shop) = make_partner_shop_with_type(ShopType::AuctionHouse);
        let shop_id = partner_shop.shop_id;
        let shops_product_id = ShopsProductId::from("failing-product".to_string());

        let body = serde_json::to_string(&vec![serde_json::json!({
            "shopsProductId": "failing-product",
            "state": "AVAILABLE"
        })])
        .unwrap();

        let event = make_event_with_body_and_key(&shop_id, &api_key, Some(body));

        let expected_partner = partner_shop.clone();
        let mut shop_service = MockGetShopService::default();
        shop_service
            .expect_verify_partner_shop()
            .return_once(move |_, _| Box::pin(async move { Ok(expected_partner) }));

        let failed_cmd = to_upsert_command(
            PutProductData {
                shops_product_id: shops_product_id.clone(),
                title: None,
                description: None,
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
                state: Some(product::data::product_state_data::ProductStateData::Available),
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
                origin_year: None,
                authenticity: Default::default(),
                condition: Default::default(),
                provenance: Default::default(),
                restoration: Default::default(),
                seller_name: None,
            },
            &partner_shop,
            partner_shop.shop_id,
            partner_shop.name.clone(),
        );

        let mut command_service = MockCommandProductService::default();
        command_service
            .expect_upsert()
            .return_once(move |_| Box::pin(async move { vec![failed_cmd] }));

        let seller_service = MockSellerService::default();

        let result = handle(event, &shop_service, &command_service, &seller_service).await;
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
        let seller_service = MockSellerService::default();

        let result = handle(event, &shop_service, &command_service, &seller_service).await;
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
        let seller_service = MockSellerService::default();

        let result = handle(event, &shop_service, &command_service, &seller_service).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().status, 400);
    }

    #[tokio::test]
    async fn should_resolve_seller_via_service_when_auction_platform_and_seller_name_provided() {
        let (api_key, mut partner_shop) = make_partner_shop_with_type(ShopType::AuctionPlatform);
        partner_shop.shop_type = ShopType::AuctionPlatform;
        let shop_id = partner_shop.shop_id;
        let resolved_seller_id = ShopId::new();
        let resolved_seller_name = ShopName::from("Resolved Seller");

        let body = serde_json::to_string(&vec![serde_json::json!({
            "shopsProductId": "test-product-1",
            "state": "AVAILABLE",
            "sellerName": "raw seller name"
        })])
        .unwrap();

        let event = make_event_with_body_and_key(&shop_id, &api_key, Some(body));

        let expected_partner = partner_shop.clone();
        let mut shop_service = MockGetShopService::default();
        shop_service
            .expect_verify_partner_shop()
            .return_once(move |_, _| Box::pin(async move { Ok(expected_partner) }));

        let expected_id = resolved_seller_id;
        let expected_name = resolved_seller_name.clone();
        let dummy_slug = common::slug_id::SlugId::from("resolved-seller");
        let mut seller_service = MockSellerService::default();
        seller_service
            .expect_get_seller_shop_details()
            .return_once(move |_| {
                Box::pin(async move { Ok((expected_id, dummy_slug, expected_name)) })
            });

        let mut command_service = MockCommandProductService::default();
        command_service.expect_upsert().return_once(move |cmds| {
            Box::pin(async move {
                assert_eq!(cmds[0].seller_id, resolved_seller_id);
                assert_eq!(cmds[0].seller_name, resolved_seller_name);
                vec![]
            })
        });

        let result = handle(event, &shop_service, &command_service, &seller_service).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().status_code, 200);
    }

    #[tokio::test]
    async fn should_resolve_seller_via_service_when_marketplace_and_seller_name_provided() {
        let (api_key, partner_shop) = make_partner_shop_with_type(ShopType::Marketplace);
        let shop_id = partner_shop.shop_id;
        let resolved_seller_id = ShopId::new();
        let resolved_seller_name = ShopName::from("Marketplace Seller");

        let body = serde_json::to_string(&vec![serde_json::json!({
            "shopsProductId": "test-product-2",
            "state": "LISTED",
            "sellerName": "marketplace seller raw"
        })])
        .unwrap();

        let event = make_event_with_body_and_key(&shop_id, &api_key, Some(body));

        let expected_partner = partner_shop.clone();
        let mut shop_service = MockGetShopService::default();
        shop_service
            .expect_verify_partner_shop()
            .return_once(move |_, _| Box::pin(async move { Ok(expected_partner) }));

        let expected_id = resolved_seller_id;
        let expected_name = resolved_seller_name.clone();
        let dummy_slug = common::slug_id::SlugId::from("marketplace-seller");
        let mut seller_service = MockSellerService::default();
        seller_service
            .expect_get_seller_shop_details()
            .return_once(move |_| {
                Box::pin(async move { Ok((expected_id, dummy_slug, expected_name)) })
            });

        let mut command_service = MockCommandProductService::default();
        command_service.expect_upsert().return_once(move |cmds| {
            Box::pin(async move {
                assert_eq!(cmds[0].seller_id, resolved_seller_id);
                assert_eq!(cmds[0].seller_name, resolved_seller_name);
                vec![]
            })
        });

        let result = handle(event, &shop_service, &command_service, &seller_service).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().status_code, 200);
    }

    #[tokio::test]
    async fn should_use_partner_shop_as_seller_when_auction_platform_without_seller_name() {
        let (api_key, partner_shop) = make_partner_shop_with_type(ShopType::AuctionPlatform);
        let shop_id = partner_shop.shop_id;
        let expected_seller_id = partner_shop.shop_id;
        let expected_seller_name = partner_shop.name.clone();

        let body = serde_json::to_string(&vec![serde_json::json!({
            "shopsProductId": "test-product-no-seller",
            "state": "AVAILABLE"
        })])
        .unwrap();

        let event = make_event_with_body_and_key(&shop_id, &api_key, Some(body));

        let expected_partner = partner_shop.clone();
        let mut shop_service = MockGetShopService::default();
        shop_service
            .expect_verify_partner_shop()
            .return_once(move |_, _| Box::pin(async move { Ok(expected_partner) }));

        // Seller service must NOT be called when seller_name is absent
        let seller_service = MockSellerService::default();

        let mut command_service = MockCommandProductService::default();
        command_service.expect_upsert().return_once(move |cmds| {
            Box::pin(async move {
                assert_eq!(cmds[0].seller_id, expected_seller_id);
                assert_eq!(cmds[0].seller_name, expected_seller_name);
                vec![]
            })
        });

        let result = handle(event, &shop_service, &command_service, &seller_service).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().status_code, 200);
    }

    #[tokio::test]
    async fn should_use_partner_shop_as_seller_when_non_platform_shop_type() {
        let (api_key, partner_shop) = make_partner_shop_with_type(ShopType::CommercialDealer);
        let shop_id = partner_shop.shop_id;
        let expected_seller_id = partner_shop.shop_id;
        let expected_seller_name = partner_shop.name.clone();

        let body = serde_json::to_string(&vec![serde_json::json!({
            "shopsProductId": "test-product-dealer",
            "state": "AVAILABLE",
            "sellerName": "ignored seller name"
        })])
        .unwrap();

        let event = make_event_with_body_and_key(&shop_id, &api_key, Some(body));

        let expected_partner = partner_shop.clone();
        let mut shop_service = MockGetShopService::default();
        shop_service
            .expect_verify_partner_shop()
            .return_once(move |_, _| Box::pin(async move { Ok(expected_partner) }));

        // Seller service must NOT be called for non-platform shop types
        let seller_service = MockSellerService::default();

        let mut command_service = MockCommandProductService::default();
        command_service.expect_upsert().return_once(move |cmds| {
            Box::pin(async move {
                assert_eq!(cmds[0].seller_id, expected_seller_id);
                assert_eq!(cmds[0].seller_name, expected_seller_name);
                vec![]
            })
        });

        let result = handle(event, &shop_service, &command_service, &seller_service).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().status_code, 200);
    }

    #[tokio::test]
    async fn should_return_500_when_seller_service_returns_error() {
        let (api_key, partner_shop) = make_partner_shop_with_type(ShopType::AuctionPlatform);
        let shop_id = partner_shop.shop_id;

        let body = serde_json::to_string(&vec![serde_json::json!({
            "shopsProductId": "test-product-err",
            "state": "AVAILABLE",
            "sellerName": "error seller"
        })])
        .unwrap();

        let event = make_event_with_body_and_key(&shop_id, &api_key, Some(body));

        let expected_partner = partner_shop.clone();
        let mut shop_service = MockGetShopService::default();
        shop_service
            .expect_verify_partner_shop()
            .return_once(move |_, _| Box::pin(async move { Ok(expected_partner) }));

        let mut seller_service = MockSellerService::default();
        seller_service
            .expect_get_seller_shop_details()
            .return_once(|_| Box::pin(async { Err(SellerServiceError::UnexpectedNone) }));

        let command_service = MockCommandProductService::default();

        let result = handle(event, &shop_service, &command_service, &seller_service).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().status, 500);
    }

    #[test]
    fn should_convert_put_product_data_to_upsert_command_for_mapping() {
        let partner_shop: PartnerShop = Faker.fake();
        let seller_id = ShopId::new();
        let seller_name = ShopName::from("Test Seller");
        let data = PutProductData {
            shops_product_id: ShopsProductId::from("test-id".to_string()),
            title: None,
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: Some(product::data::product_state_data::ProductStateData::Listed),
            url: None,
            images: None,
            auction_start: None,
            auction_end: None,
            origin_year: None,
            authenticity: Default::default(),
            condition: Default::default(),
            provenance: Default::default(),
            restoration: Default::default(),
            seller_name: None,
        };

        let cmd = to_upsert_command(data, &partner_shop, seller_id, seller_name.clone());

        assert_eq!(cmd.shop_id, partner_shop.shop_id);
        assert_eq!(
            cmd.shops_product_id,
            ShopsProductId::from("test-id".to_string())
        );
        assert!(cmd.native_title.is_none());
        assert!(cmd.native_price.is_none());
        assert_eq!(
            cmd.state,
            Some(common::product_state::domain::ProductState::Listed)
        );
        assert_eq!(cmd.seller_id, seller_id);
        assert_eq!(cmd.seller_name, seller_name);
    }

    #[test]
    fn should_convert_put_product_data_with_all_fields_to_upsert_command_for_mapping() {
        let partner_shop: PartnerShop = Faker.fake();
        let seller_id = ShopId::new();
        let seller_name = ShopName::from("Full Seller");
        let data = PutProductData {
            shops_product_id: ShopsProductId::from("test-id".to_string()),
            title: Some(common::language::data::LocalizedTextData::new(
                "Test Title",
                common::language::data::LanguageData::De,
            )),
            description: Some(common::language::data::LocalizedTextData::new(
                "Test Description",
                common::language::data::LanguageData::De,
            )),
            price: Some(common::price::data::PriceData::new(
                common::currency::data::CurrencyData::Eur,
                1000,
            )),
            price_estimate_min: None,
            price_estimate_max: None,
            state: Some(product::data::product_state_data::ProductStateData::Available),
            url: Some(url::Url::parse("https://example.com").unwrap()),
            images: Some(vec![
                url::Url::parse("https://example.com/img.jpg").unwrap(),
            ]),
            auction_start: None,
            auction_end: None,
            origin_year: None,
            authenticity: Default::default(),
            condition: Default::default(),
            provenance: Default::default(),
            restoration: Default::default(),
            seller_name: Some("Full Seller".to_string()),
        };

        let cmd = to_upsert_command(data, &partner_shop, seller_id, seller_name.clone());

        assert_eq!(cmd.shop_id, partner_shop.shop_id);
        assert!(cmd.native_title.is_some());
        assert!(cmd.native_description.is_some());
        assert!(cmd.native_price.is_some());
        assert_eq!(
            cmd.state,
            Some(common::product_state::domain::ProductState::Available)
        );
        assert!(cmd.url.is_some());
        assert_eq!(cmd.images.len(), 1);
        assert_eq!(cmd.seller_id, seller_id);
        assert_eq!(cmd.seller_name, seller_name);
    }
}
