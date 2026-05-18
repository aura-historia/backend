use crate::woocommerce::types::{
    WoocommerceProductEvent, WoocommerceProductEventKind, WoocommerceProductPayload,
};
use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use base64::Engine;
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::{BAD_BODY_VALUE, BAD_HEADER_VALUE, INTERNAL_SERVER_ERROR};
use common::shop_id::api::extract_shop_id_path;
use lambda_runtime::LambdaEvent;
use openssl::hash::MessageDigest;
use openssl::memcmp;
use openssl::pkey::PKey;
use openssl::sign::Signer;
use product::service::command_service::CommandProductService;
use product::service::product_command::UpsertProductCommand;
use serde_json::json;
use shop::core::partner_shop::PartnerShop;
use shop::core::partner_shop_api_key::api::extract_api_key;
use shop::service::get_service::GetShopService;

pub const WOOCOMMERCE_TOPIC_PRODUCT_CREATED: &str = "product.created";
pub const WOOCOMMERCE_TOPIC_PRODUCT_UPDATED: &str = "product.updated";
pub const WOOCOMMERCE_TOPIC_PRODUCT_DELETED: &str = "product.deleted";
const WOOCOMMERCE_TOPIC_HEADER: &str = "x-wc-webhook-topic";
const WOOCOMMERCE_SIGNATURE_HEADER: &str = "x-wc-webhook-signature";

pub async fn handle_woocommerce(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    get_shop_service: &(impl GetShopService + Sync),
    command_product_service: &(impl CommandProductService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let shop_id = extract_shop_id_path(&event.payload.path_parameters)?;
    let api_key = extract_api_key(&event.payload)?;
    let partner_shop = get_shop_service
        .verify_partner_shop(&api_key, &shop_id)
        .await?;

    let body = body_bytes(&event.payload)?;
    verify_signature(&event.payload, &body, &partner_shop)?;

    let kind = extract_event_kind(&event.payload)?;
    let payload: WoocommerceProductPayload = serde_json::from_slice(&body).map_err(|err| {
        let msg = err.to_string();
        ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(msg)
    })?;

    let command = UpsertProductCommand::try_from(WoocommerceProductEvent {
        shop: partner_shop,
        kind,
        payload,
    })
    .map_err(|err| ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)))?;

    if command_product_service.upsert(command).await.is_some() {
        return Err(ApiError::internal_server_error(
            INTERNAL_SERVER_ERROR,
            "Failed to upsert WooCommerce product.".into(),
        ));
    }

    Ok(ApiGatewayV2HttpResponseBuilder::json(200).build())
}

fn body_bytes(request: &ApiGatewayV2httpRequest) -> Result<Vec<u8>, ApiError> {
    let body = request
        .body
        .as_deref()
        .filter(|body| !body.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request(BAD_BODY_VALUE, "Body cannot be empty.".into())
                .with_detail("Body cannot be empty.")
        })?;

    if request.is_base64_encoded {
        base64::engine::general_purpose::STANDARD
            .decode(body)
            .map_err(|err| ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)))
    } else {
        Ok(body.as_bytes().to_vec())
    }
}

fn extract_event_kind(
    request: &ApiGatewayV2httpRequest,
) -> Result<WoocommerceProductEventKind, ApiError> {
    let topic = request
        .headers
        .get(WOOCOMMERCE_TOPIC_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            ApiError::bad_request(BAD_HEADER_VALUE, "Missing WooCommerce topic header.".into())
                .with_header_field(WOOCOMMERCE_TOPIC_HEADER)
        })?;

    match topic {
        WOOCOMMERCE_TOPIC_PRODUCT_CREATED => Ok(WoocommerceProductEventKind::Create),
        WOOCOMMERCE_TOPIC_PRODUCT_UPDATED => Ok(WoocommerceProductEventKind::Update),
        WOOCOMMERCE_TOPIC_PRODUCT_DELETED => Ok(WoocommerceProductEventKind::Delete),
        _ => Err(ApiError::bad_request(
            BAD_HEADER_VALUE,
            format!("Unsupported WooCommerce topic '{topic}'.").into(),
        )
        .with_header_field(WOOCOMMERCE_TOPIC_HEADER)),
    }
}

fn verify_signature(
    request: &ApiGatewayV2httpRequest,
    body: &[u8],
    partner_shop: &PartnerShop,
) -> Result<(), ApiError> {
    let secret = partner_shop
        .woocommerce_webhook_secret
        .as_ref()
        .ok_or_else(|| {
            let msg = format!(
                "Shop with id '{}' has no woocommerce webhook secret configured.",
                partner_shop.shop_id
            );
            ApiError::internal_server_error(INTERNAL_SERVER_ERROR, msg.clone().into())
                .with_detail(msg)
        })?;
    let signature = request
        .headers
        .get(WOOCOMMERCE_SIGNATURE_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            ApiError::unauthorized(BAD_HEADER_VALUE)
                .with_header_field(WOOCOMMERCE_SIGNATURE_HEADER)
                .with_detail("Missing WooCommerce signature header.")
        })?;
    let signature = base64::engine::general_purpose::STANDARD
        .decode(signature)
        .map_err(|err| {
            ApiError::unauthorized(BAD_HEADER_VALUE)
                .with_header_field(WOOCOMMERCE_SIGNATURE_HEADER)
                .with_detail(err.to_string())
        })?;

    let key = PKey::hmac(secret.as_ref().as_bytes())
        .map_err(|err| ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err)))?;
    let mut signer = Signer::new(MessageDigest::sha256(), &key)
        .map_err(|err| ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err)))?;
    signer
        .update(body)
        .map_err(|err| ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err)))?;
    let expected = signer
        .sign_to_vec()
        .map_err(|err| ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(err)))?;
    if memcmp::eq(&expected, &signature) {
        Ok(())
    } else {
        Err(ApiError::unauthorized(BAD_HEADER_VALUE)
            .with_header_field(WOOCOMMERCE_SIGNATURE_HEADER)
            .with_detail("WooCommerce signature mismatch."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
    use base64::Engine;
    use common::currency::domain::Currency;
    use common::price::domain::MonetaryAmount;
    use common::product_state::domain::ProductState;
    use fake::{Fake, Faker};
    use http::HeaderMap;
    use lambda_runtime::{Context, LambdaEvent};
    use openssl::hash::MessageDigest;
    use openssl::pkey::PKey;
    use openssl::sign::Signer;
    use product::service::command_service::MockCommandProductService;
    use shop::core::partner_shop::PartnerShop;
    use shop::core::partner_shop_api_key::{HashedPartnerShopApiKey, PartnerShopApiKey};
    use shop::core::woocommerce_webhook_secret::WoocommerceWebhookSecret;
    use shop::service::get_service::MockGetShopService;

    const SECRET: &str = "woocommerce-secret";

    fn signature(body: &str) -> String {
        let key = PKey::hmac(SECRET.as_bytes()).unwrap();
        let mut signer = Signer::new(MessageDigest::sha256(), &key).unwrap();
        signer.update(body.as_bytes()).unwrap();
        base64::engine::general_purpose::STANDARD.encode(signer.sign_to_vec().unwrap())
    }

    fn partner_shop(api_key: &PartnerShopApiKey) -> PartnerShop {
        let mut shop: PartnerShop = Faker.fake();
        let hashed: HashedPartnerShopApiKey = api_key.clone().into();
        shop.hashed_api_key = Some(hashed);
        shop.woocommerce_webhook_secret = Some(WoocommerceWebhookSecret::from(SECRET));
        shop.woocommerce_currency = Some(Currency::Eur);
        shop.woocommerce_language = Some(common::language::domain::Language::En);
        shop
    }

    fn event(
        shop: &PartnerShop,
        api_key: &PartnerShopApiKey,
        topic: &str,
        body: &str,
        signature: String,
    ) -> LambdaEvent<ApiGatewayV2httpRequest> {
        let key: String = api_key.clone().into();
        let mut request = ApiGatewayV2httpRequest::default();
        request.route_key = Some("POST /api/v1/webhooks/woocommerce/{shopId}".to_owned());
        request
            .path_parameters
            .insert("shopId".to_owned(), shop.shop_id.to_string());
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", key.parse().unwrap());
        headers.insert(WOOCOMMERCE_TOPIC_HEADER, topic.parse().unwrap());
        headers.insert(WOOCOMMERCE_SIGNATURE_HEADER, signature.parse().unwrap());
        request.headers = headers;
        request.body = Some(body.to_owned());
        LambdaEvent::new(request, Context::default())
    }

    fn product_body(price: &str) -> String {
        serde_json::json!({
            "id": 17,
            "name": "Test Produkt Titel",
            "permalink": "http://aura-historia-test.local/product/test-produkt-titel/",
            "description": "<p>Hayde yallah test beschreibung</p>\n",
            "price": price,
            "status": "publish",
            "stock_status": "instock",
            "images": []
        })
        .to_string()
    }

    #[tokio::test]
    async fn should_upsert_product_when_created_webhook_is_valid() {
        let api_key = PartnerShopApiKey::new();
        let shop = partner_shop(&api_key);
        let body = product_body("42.69");
        let lambda_event = event(
            &shop,
            &api_key,
            WOOCOMMERCE_TOPIC_PRODUCT_CREATED,
            &body,
            signature(&body),
        );

        let expected_shop = shop.clone();
        let mut get_shop_service = MockGetShopService::default();
        get_shop_service
            .expect_verify_partner_shop()
            .return_once(move |_, _| Box::pin(async move { Ok(expected_shop) }));

        let mut product_service = MockCommandProductService::default();
        product_service.expect_upsert().return_once(move |cmd| {
            Box::pin(async move {
                assert_eq!(cmd.shop_id, shop.shop_id);
                assert_eq!(cmd.shops_product_id.to_string(), "17");
                assert_eq!(cmd.state, Some(ProductState::Available));
                assert_eq!(
                    cmd.native_price.as_ref().map(|price| price.monetary_amount),
                    Some(MonetaryAmount::from(4269_u64))
                );
                None
            })
        });

        let response = handle_woocommerce(lambda_event, &get_shop_service, &product_service)
            .await
            .unwrap();
        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_upsert_removed_product_when_deleted_webhook_is_valid() {
        let api_key = PartnerShopApiKey::new();
        let shop = partner_shop(&api_key);
        let body = serde_json::json!({ "id": 17 }).to_string();
        let lambda_event = event(
            &shop,
            &api_key,
            WOOCOMMERCE_TOPIC_PRODUCT_DELETED,
            &body,
            signature(&body),
        );

        let expected_shop = shop.clone();
        let mut get_shop_service = MockGetShopService::default();
        get_shop_service
            .expect_verify_partner_shop()
            .return_once(move |_, _| Box::pin(async move { Ok(expected_shop) }));

        let mut product_service = MockCommandProductService::default();
        product_service.expect_upsert().return_once(move |cmd| {
            Box::pin(async move {
                assert_eq!(cmd.state, Some(ProductState::Removed));
                assert!(cmd.native_title.is_none());
                None
            })
        });

        let response = handle_woocommerce(lambda_event, &get_shop_service, &product_service)
            .await
            .unwrap();
        assert_eq!(200, response.status_code);
    }

    #[tokio::test]
    async fn should_return_error_when_product_upsert_fails() {
        let api_key = PartnerShopApiKey::new();
        let shop = partner_shop(&api_key);
        let body = product_body("42.69");
        let lambda_event = event(
            &shop,
            &api_key,
            WOOCOMMERCE_TOPIC_PRODUCT_CREATED,
            &body,
            signature(&body),
        );

        let expected_shop = shop.clone();
        let mut get_shop_service = MockGetShopService::default();
        get_shop_service
            .expect_verify_partner_shop()
            .return_once(move |_, _| Box::pin(async move { Ok(expected_shop) }));

        let mut product_service = MockCommandProductService::default();
        product_service
            .expect_upsert()
            .return_once(|cmd| Box::pin(async move { Some(cmd) }));

        let actual = handle_woocommerce(lambda_event, &get_shop_service, &product_service).await;

        assert!(actual.is_err());
    }

    #[tokio::test]
    async fn should_return_401_when_signature_is_invalid() {
        let api_key = PartnerShopApiKey::new();
        let shop = partner_shop(&api_key);
        let body = product_body("42.69");
        let lambda_event = event(
            &shop,
            &api_key,
            WOOCOMMERCE_TOPIC_PRODUCT_UPDATED,
            &body,
            "bad-signature".to_owned(),
        );

        let expected_shop = shop.clone();
        let mut get_shop_service = MockGetShopService::default();
        get_shop_service
            .expect_verify_partner_shop()
            .return_once(move |_, _| Box::pin(async move { Ok(expected_shop) }));

        let err = handle_woocommerce(
            lambda_event,
            &get_shop_service,
            &MockCommandProductService::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(401, err.status);
    }

    #[tokio::test]
    async fn should_return_400_when_topic_is_unsupported() {
        let api_key = PartnerShopApiKey::new();
        let shop = partner_shop(&api_key);
        let body = product_body("42.69");
        let lambda_event = event(&shop, &api_key, "coupon.created", &body, signature(&body));

        let expected_shop = shop.clone();
        let mut get_shop_service = MockGetShopService::default();
        get_shop_service
            .expect_verify_partner_shop()
            .return_once(move |_, _| Box::pin(async move { Ok(expected_shop) }));

        let err = handle_woocommerce(
            lambda_event,
            &get_shop_service,
            &MockCommandProductService::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(400, err.status);
    }
}
