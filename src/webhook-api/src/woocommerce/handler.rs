use crate::woocommerce::types::{
    WoocommerceProductEvent, WoocommerceProductEventKind, WoocommerceProductPayload,
};
use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use base64::Engine;
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::ApiError;
use common::api::error_code::{
    BAD_BODY_VALUE, BAD_HEADER_VALUE, INTERNAL_SERVER_ERROR, PARTNER_SHOP_NOT_PARTNERED,
    SERVICE_UNAVAILABLE, UNAUTHORIZED,
};
use common::shop_id::api::extract_shop_id_path;
use lambda_runtime::LambdaEvent;
use openssl::hash::MessageDigest;
use openssl::memcmp;
use openssl::pkey::PKey;
use openssl::sign::Signer;
use product_lambda_ingest_partner_products::{AsyncProductCommandData, AsyncProductCommandService};
use shop::core::partner_shop::PartnerShop;
use shop::service::get_service::GetShopService;
use user::service::authenticator_service::{AuthenticatedPrincipal, AuthenticatorService};
use user::service::user_service::UserService;

pub const WOOCOMMERCE_TOPIC_PRODUCT_CREATED: &str = "product.created";
pub const WOOCOMMERCE_TOPIC_PRODUCT_UPDATED: &str = "product.updated";
pub const WOOCOMMERCE_TOPIC_PRODUCT_DELETED: &str = "product.deleted";
const WOOCOMMERCE_TOPIC_HEADER: &str = "x-wc-webhook-topic";
const WOOCOMMERCE_SIGNATURE_HEADER: &str = "x-wc-webhook-signature";

pub async fn handle_woocommerce(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    get_shop_service: &(impl GetShopService + Sync),
    user_service: &(impl UserService + Sync),
    authenticator_service: &(impl AuthenticatorService + Sync),
    async_product_command_service: &(impl AsyncProductCommandService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    let shop_id = extract_shop_id_path(&event.payload.path_parameters)?;

    let authenticated = authenticator_service
        .authenticate(&event.payload.headers)
        .await?
        .ok_or_else(|| ApiError::unauthorized(UNAUTHORIZED).with_header_field("Authorization"))?;

    let user_id = match authenticated {
        AuthenticatedPrincipal::UserId(user_id) => user_id,
        AuthenticatedPrincipal::AccessToken(access_token) => access_token.user_id,
    };

    let user = user_service.find_user(&user_id).await?;
    if !user.partner_shops.contains(&shop_id) {
        return Err(
            ApiError::forbidden(PARTNER_SHOP_NOT_PARTNERED).with_detail(format!(
                "User '{}' is not the partner of shop '{}'",
                user_id, shop_id
            )),
        );
    }

    let partner_shop = get_shop_service.find_partner_shop(&shop_id).await?;

    let body = body_bytes(&event.payload)?;
    verify_signature(&event.payload, &body, &partner_shop)?;

    let kind = extract_event_kind(&event.payload)?;
    let payload: WoocommerceProductPayload = serde_json::from_slice(&body).map_err(|err| {
        let msg = err.to_string();
        ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)).with_detail(msg)
    })?;

    let command = AsyncProductCommandData::try_from(WoocommerceProductEvent {
        shop: partner_shop,
        kind,
        payload,
    })
    .map_err(|err| ApiError::bad_request(BAD_BODY_VALUE, Box::new(err)))?;

    let failures = async_product_command_service.send(vec![command]).await;
    if let Some(failure) = failures.first() {
        return Err(ApiError::service_unavailable(
            SERVICE_UNAVAILABLE,
            failure.error.clone().into(),
        ));
    }

    Ok(ApiGatewayV2HttpResponseBuilder::new(202).build())
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
    use common::shop_id::ShopId;
    use common::user_id::UserId;
    use fake::{Fake, Faker};
    use http::HeaderMap;
    use lambda_runtime::{Context, LambdaEvent};
    use openssl::hash::MessageDigest;
    use openssl::pkey::PKey;
    use openssl::sign::Signer;
    use product_lambda_ingest_partner_products::service::MockAsyncProductCommandService;
    use shop::core::partner_shop::PartnerShop;
    use shop::core::woocommerce_webhook_secret::WoocommerceWebhookSecret;
    use shop::service::get_service::MockGetShopService;
    use user::core::access_token::AccessToken;
    use user::service::authenticator_service::{AuthenticatedPrincipal, MockAuthenticatorService};
    use user::service::user_service::MockUserService;

    const SECRET: &str = "woocommerce-secret";

    fn signature(body: &str) -> String {
        let key = PKey::hmac(SECRET.as_bytes()).unwrap();
        let mut signer = Signer::new(MessageDigest::sha256(), &key).unwrap();
        signer.update(body.as_bytes()).unwrap();
        base64::engine::general_purpose::STANDARD.encode(signer.sign_to_vec().unwrap())
    }

    fn partner_shop(shop_id: ShopId) -> PartnerShop {
        let mut shop: PartnerShop = Faker.fake();
        shop.shop_id = shop_id;
        shop.woocommerce_webhook_secret = Some(WoocommerceWebhookSecret::from(SECRET));
        shop.woocommerce_currency = Some(Currency::Eur);
        shop.woocommerce_language = Some(common::language::domain::Language::En);
        shop
    }

    fn make_access_token(user_id: UserId) -> AccessToken {
        let mut token: AccessToken = Faker.fake();
        token.user_id = user_id;
        token
    }

    fn authorized_services(shop_id: ShopId) -> (MockAuthenticatorService, MockUserService) {
        let user_id = UserId::new();
        let access_token = make_access_token(user_id);

        let mut authenticator = MockAuthenticatorService::default();
        authenticator.expect_authenticate().return_once(move |_| {
            Box::pin(async move { Ok(Some(AuthenticatedPrincipal::AccessToken(access_token))) })
        });

        let mut user_service = MockUserService::default();
        user_service.expect_find_user().return_once(move |_| {
            let mut user: user::core::user::User = Faker.fake();
            user.user_id = user_id;
            user.partner_shops.insert(shop_id);
            Box::pin(async move { Ok(user) })
        });

        (authenticator, user_service)
    }

    fn event(
        shop: &PartnerShop,
        topic: &str,
        body: &str,
        signature: String,
    ) -> LambdaEvent<ApiGatewayV2httpRequest> {
        let mut request = ApiGatewayV2httpRequest::default();
        request.route_key = Some("POST /api/v1/webhooks/woocommerce/{shopId}".to_owned());
        request
            .path_parameters
            .insert("shopId".to_owned(), shop.shop_id.to_string());
        let mut headers = HeaderMap::new();
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
        let shop_id = ShopId::new();
        let shop = partner_shop(shop_id);
        let body = product_body("42.69");
        let lambda_event = event(
            &shop,
            WOOCOMMERCE_TOPIC_PRODUCT_CREATED,
            &body,
            signature(&body),
        );

        let (authenticator, user_service) = authorized_services(shop_id);

        let expected_shop = shop.clone();
        let mut get_shop_service = MockGetShopService::default();
        get_shop_service
            .expect_find_partner_shop()
            .return_once(move |_| Box::pin(async move { Ok(expected_shop) }));

        let mut product_service = MockAsyncProductCommandService::default();
        product_service.expect_send().return_once(move |cmds| {
            Box::pin(async move {
                assert_eq!(cmds.len(), 1);
                let AsyncProductCommandData::Upsert(cmd) = &cmds[0] else {
                    panic!("Expected upsert command")
                };
                assert_eq!(cmd.shop_id, shop.shop_id);
                assert_eq!(cmd.shops_product_id.to_string(), "17");
                assert_eq!(cmd.state, Some(ProductState::Available.into()));
                assert_eq!(
                    cmd.price
                        .as_ref()
                        .map(|price| common::price::domain::Price::from(*price).monetary_amount),
                    Some(MonetaryAmount::from(4269_u64))
                );
                vec![]
            })
        });

        let response = handle_woocommerce(
            lambda_event,
            &get_shop_service,
            &user_service,
            &authenticator,
            &product_service,
        )
        .await
        .unwrap();
        assert_eq!(202, response.status_code);
    }

    #[tokio::test]
    async fn should_update_removed_product_when_deleted_webhook_is_valid() {
        let shop_id = ShopId::new();
        let mut shop = partner_shop(shop_id);
        shop.woocommerce_language = None;
        let body = serde_json::json!({ "id": 17 }).to_string();
        let lambda_event = event(
            &shop,
            WOOCOMMERCE_TOPIC_PRODUCT_DELETED,
            &body,
            signature(&body),
        );

        let (authenticator, user_service) = authorized_services(shop_id);

        let expected_shop = shop.clone();
        let mut get_shop_service = MockGetShopService::default();
        get_shop_service
            .expect_find_partner_shop()
            .return_once(move |_| Box::pin(async move { Ok(expected_shop) }));

        let mut product_service = MockAsyncProductCommandService::default();
        product_service.expect_send().return_once(move |cmds| {
            Box::pin(async move {
                let AsyncProductCommandData::Update(cmd) = &cmds[0] else {
                    panic!("Expected update command")
                };
                assert_eq!(cmd.state, Some(ProductState::Removed.into()));
                assert!(cmd.url.is_none());
                assert!(cmd.images.is_none());
                vec![]
            })
        });

        let response = handle_woocommerce(
            lambda_event,
            &get_shop_service,
            &user_service,
            &authenticator,
            &product_service,
        )
        .await
        .unwrap();
        assert_eq!(202, response.status_code);
    }

    #[tokio::test]
    async fn should_return_401_when_unauthenticated() {
        let shop_id = ShopId::new();
        let shop = partner_shop(shop_id);
        let body = product_body("42.69");
        let lambda_event = event(
            &shop,
            WOOCOMMERCE_TOPIC_PRODUCT_UPDATED,
            &body,
            signature(&body),
        );

        let mut authenticator = MockAuthenticatorService::default();
        authenticator
            .expect_authenticate()
            .return_once(|_| Box::pin(async { Ok(None) }));

        let err = handle_woocommerce(
            lambda_event,
            &MockGetShopService::default(),
            &MockUserService::default(),
            &authenticator,
            &MockAsyncProductCommandService::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(401, err.status);
    }

    #[tokio::test]
    async fn should_return_401_when_signature_is_invalid() {
        let shop_id = ShopId::new();
        let shop = partner_shop(shop_id);
        let body = product_body("42.69");
        let lambda_event = event(
            &shop,
            WOOCOMMERCE_TOPIC_PRODUCT_UPDATED,
            &body,
            "bad-signature".to_owned(),
        );

        let (authenticator, user_service) = authorized_services(shop_id);

        let expected_shop = shop.clone();
        let mut get_shop_service = MockGetShopService::default();
        get_shop_service
            .expect_find_partner_shop()
            .return_once(move |_| Box::pin(async move { Ok(expected_shop) }));

        let err = handle_woocommerce(
            lambda_event,
            &get_shop_service,
            &user_service,
            &authenticator,
            &MockAsyncProductCommandService::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(401, err.status);
    }

    #[tokio::test]
    async fn should_return_403_when_user_is_not_partner_of_shop() {
        let shop_id = ShopId::new();
        let shop = partner_shop(shop_id);
        let body = product_body("42.69");
        let lambda_event = event(
            &shop,
            WOOCOMMERCE_TOPIC_PRODUCT_CREATED,
            &body,
            signature(&body),
        );

        let user_id = UserId::new();
        let access_token = make_access_token(user_id);

        let mut authenticator = MockAuthenticatorService::default();
        authenticator.expect_authenticate().return_once(move |_| {
            Box::pin(async move { Ok(Some(AuthenticatedPrincipal::AccessToken(access_token))) })
        });

        let mut user_service = MockUserService::default();
        user_service.expect_find_user().return_once(move |_| {
            let mut user: user::core::user::User = Faker.fake();
            user.user_id = user_id;
            // partner_shops does NOT contain shop_id
            user.partner_shops.clear();
            Box::pin(async move { Ok(user) })
        });

        let err = handle_woocommerce(
            lambda_event,
            &MockGetShopService::default(),
            &user_service,
            &authenticator,
            &MockAsyncProductCommandService::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(403, err.status);
    }

    #[tokio::test]
    async fn should_return_400_when_topic_is_unsupported() {
        let shop_id = ShopId::new();
        let shop = partner_shop(shop_id);
        let body = product_body("42.69");
        let lambda_event = event(&shop, "coupon.created", &body, signature(&body));

        let (authenticator, user_service) = authorized_services(shop_id);

        let expected_shop = shop.clone();
        let mut get_shop_service = MockGetShopService::default();
        get_shop_service
            .expect_find_partner_shop()
            .return_once(move |_| Box::pin(async move { Ok(expected_shop) }));

        let err = handle_woocommerce(
            lambda_event,
            &get_shop_service,
            &user_service,
            &authenticator,
            &MockAsyncProductCommandService::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(400, err.status);
    }
}
