use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use base64::Engine;
use common::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use common::api::error::{ApiError, log_api_error};
use common::api::error_code::{BAD_BODY_VALUE, BAD_HEADER_VALUE, INTERNAL_SERVER_ERROR};
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{MonetaryAmount, Price};
use common::product_state::domain::ProductState;
use common::shop_id::api::extract_shop_id_path;
use common::shops_product_id::ShopsProductId;
use lambda_runtime::LambdaEvent;
use lingua::{Language as LinguaLanguage, LanguageDetector, LanguageDetectorBuilder};
use openssl::hash::MessageDigest;
use openssl::memcmp;
use openssl::pkey::PKey;
use openssl::sign::Signer;
use product::core::description::Description;
use product::core::product_image::ProductImage;
use product::core::prohibited_content::ProhibitedContent;
use product::core::title::Title;
use product::service::command_service::CommandProductService;
use product::service::product_command::UpsertProductCommand;
use serde::Deserialize;
use serde_json::json;
use shop::core::partner_shop::PartnerShop;
use shop::core::partner_shop_api_key::api::extract_api_key;
use shop::service::get_service::GetShopService;
use std::sync::OnceLock;
use tracing::warn;
use url::Url;

pub const WOOCOMMERCE_TOPIC_PRODUCT_CREATED: &str = "product.created";
pub const WOOCOMMERCE_TOPIC_PRODUCT_UPDATED: &str = "product.updated";
pub const WOOCOMMERCE_TOPIC_PRODUCT_DELETED: &str = "product.deleted";
const WOOCOMMERCE_TOPIC_HEADER: &str = "x-wc-webhook-topic";
const WOOCOMMERCE_SIGNATURE_HEADER: &str = "x-wc-webhook-signature";

#[derive(Debug, Clone, Deserialize)]
pub struct WoocommerceProductPayload {
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub permalink: Option<Url>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub short_description: Option<String>,
    #[serde(default)]
    pub price: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub stock_status: Option<String>,
    #[serde(default)]
    pub images: Vec<WoocommerceImagePayload>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WoocommerceImagePayload {
    pub src: Url,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WoocommerceProductEventKind {
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone)]
pub struct WoocommerceProductEvent {
    pub shop: PartnerShop,
    pub kind: WoocommerceProductEventKind,
    pub payload: WoocommerceProductPayload,
}

#[derive(Debug, thiserror::Error)]
pub enum WoocommerceProductEventError {
    #[error("Missing product title")]
    MissingTitle,
    #[error("Missing product URL")]
    MissingUrl,
    #[error("Invalid WooCommerce price '{0}'")]
    InvalidPrice(String),
    #[error("Shop has no currency configured")]
    MissingCurrency,
}

#[tracing::instrument(
    skip(event, get_shop_service, command_product_service),
    fields(
        requestId = %event.context.request_id,
        method = event.payload.request_context.http.method.as_str(),
        path = &event.payload.raw_path.as_deref().unwrap_or("NULL"),
        ip = &event.payload.request_context.http.source_ip.as_deref().unwrap_or("NULL"),
        userAgent = &event.payload.request_context.http.user_agent.as_deref().unwrap_or("NULL"),
    )
)]
pub async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    get_shop_service: &(impl GetShopService + Sync),
    command_product_service: &(impl CommandProductService + Sync),
) -> Result<ApiGatewayV2httpResponse, lambda_runtime::Error> {
    match handle(event, get_shop_service, command_product_service).await {
        Ok(response) => Ok(response),
        Err(err) => {
            log_api_error(&err);
            Ok(ApiGatewayV2httpResponse::from(err))
        }
    }
}

pub async fn handle(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    get_shop_service: &(impl GetShopService + Sync),
    command_product_service: &(impl CommandProductService + Sync),
) -> Result<ApiGatewayV2httpResponse, ApiError> {
    match event.payload.route_key.as_deref() {
        Some("POST /api/v1/webhooks/woocommerce/{shopId}") => {
            handle_woocommerce(event, get_shop_service, command_product_service).await
        }
        Some(unknown) => Err(ApiError::internal_server_error(
            INTERNAL_SERVER_ERROR,
            format!("Unknown route-key '{unknown}' in AWS-Payload").into(),
        )),
        None => Err(ApiError::internal_server_error(
            INTERNAL_SERVER_ERROR,
            "Missing route-key in AWS-Payload".into(),
        )),
    }
}

async fn handle_woocommerce(
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

    let errors = command_product_service.upsert(vec![command]).await;
    Ok(ApiGatewayV2HttpResponseBuilder::json(200)
        .body_serde(json!({ "errors": errors.len() }))?
        .build())
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
        .ok_or_else(|| ApiError::unauthorized(BAD_HEADER_VALUE).with_detail("Missing WooCommerce webhook secret for shop."))?;
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
        Err(
        ApiError::unauthorized(BAD_HEADER_VALUE)
            .with_header_field(WOOCOMMERCE_SIGNATURE_HEADER)
            .with_detail("WooCommerce signature mismatch.")
        )
    }
}

impl TryFrom<WoocommerceProductEvent> for UpsertProductCommand {
    type Error = WoocommerceProductEventError;

    fn try_from(event: WoocommerceProductEvent) -> Result<Self, Self::Error> {
        let title = event
            .payload
            .name
            .as_deref()
            .filter(|title| !title.trim().is_empty());
        let description = event
            .payload
            .description
            .as_deref()
            .or(event.payload.short_description.as_deref())
            .map(html_to_text)
            .filter(|description| !description.is_empty());
        let language = infer_language(description.as_deref(), title);
        let state = match event.kind {
            WoocommerceProductEventKind::Delete => ProductState::Removed,
            WoocommerceProductEventKind::Create | WoocommerceProductEventKind::Update => {
                product_state(&event.payload)
            }
        };
        let native_title = match event.kind {
            WoocommerceProductEventKind::Delete => title.map(|title| {
                Localized::new(language, Title::from(title))
            }),
            _ => Some(Localized::new(
                language,
                Title::from(title.ok_or(WoocommerceProductEventError::MissingTitle)?),
            )),
        };
        let url = match event.kind {
            WoocommerceProductEventKind::Delete => event.payload.permalink,
            _ => Some(
                event
                    .payload
                    .permalink
                    .ok_or(WoocommerceProductEventError::MissingUrl)?,
            ),
        };

        Ok(UpsertProductCommand {
            shop_id: event.shop.shop_id,
            shops_product_id: ShopsProductId::from(event.payload.id.to_string()),
            seller_name_raw: None,
            structured_address: None,
            geo_address: None,
            native_title,
            native_description: description
                .map(Description::from)
                .map(|description| Localized::new(language, description)),
            native_price: parse_price(event.payload.price.as_deref(), event.shop.shopify_currency)?,
            native_price_estimate_min: None,
            native_price_estimate_max: None,
            state: Some(state),
            url,
            images: event
                .payload
                .images
                .into_iter()
                .map(|image| ProductImage {
                    url: image.src,
                    prohibited_content: ProhibitedContent::Unknown,
                })
                .collect(),
            auction_start: None,
            auction_end: None,
        })
    }
}

pub fn html_to_text(html: &str) -> String {
    html2text::from_read(html.as_bytes(), 120)
        .unwrap_or_else(|_| String::new())
        .trim()
        .to_owned()
}

pub fn product_state(payload: &WoocommerceProductPayload) -> ProductState {
    match payload.status.as_deref() {
        Some("publish") => match payload.stock_status.as_deref() {
            Some("outofstock") => ProductState::Sold,
            _ => ProductState::Available,
        },
        Some("draft") | Some("pending") | Some("private") => ProductState::Listed,
        Some("trash") => ProductState::Removed,
        Some(other) => {
            warn!(woocommerceStatus = %other, "Unknown WooCommerce product status.");
            ProductState::Unknown
        }
        None => ProductState::Unknown,
    }
}

pub fn parse_price(
    price: Option<&str>,
    currency: Option<common::currency::domain::Currency>,
) -> Result<Option<Price>, WoocommerceProductEventError> {
    let Some(price) = price.filter(|price| !price.trim().is_empty()) else {
        return Ok(None);
    };
    let currency = currency.ok_or(WoocommerceProductEventError::MissingCurrency)?;
    let trimmed = price.trim();
    let (major, minor) = trimmed.split_once('.').unwrap_or((trimmed, ""));
    if !major.chars().all(|c| c.is_ascii_digit()) || !minor.chars().all(|c| c.is_ascii_digit()) {
        return Err(WoocommerceProductEventError::InvalidPrice(
            trimmed.to_owned(),
        ));
    }
    let major: u64 = major
        .parse()
        .map_err(|_| WoocommerceProductEventError::InvalidPrice(trimmed.to_owned()))?;
    let mut minor = minor.chars().take(2).collect::<String>();
    while minor.len() < 2 {
        minor.push('0');
    }
    let minor: u64 = minor
        .parse()
        .map_err(|_| WoocommerceProductEventError::InvalidPrice(trimmed.to_owned()))?;
    Ok(Some(Price::new(
        MonetaryAmount::from(major * 100 + minor),
        currency,
    )))
}

pub fn infer_language(description: Option<&str>, title: Option<&str>) -> Language {
    description
        .filter(|text| !text.trim().is_empty())
        .and_then(detect_language)
        .or_else(|| {
            title
                .filter(|text| !text.trim().is_empty())
                .and_then(detect_language)
        })
        .unwrap_or(Language::En)
}

fn detect_language(text: &str) -> Option<Language> {
    static DETECTOR: OnceLock<LanguageDetector> = OnceLock::new();
    let detector = DETECTOR.get_or_init(|| {
        LanguageDetectorBuilder::from_languages(&[
            LinguaLanguage::English,
            LinguaLanguage::German,
            LinguaLanguage::French,
            LinguaLanguage::Spanish,
            LinguaLanguage::Italian,
            LinguaLanguage::Chinese,
            LinguaLanguage::Portuguese,
            LinguaLanguage::Polish,
            LinguaLanguage::Turkish,
            LinguaLanguage::Dutch,
            LinguaLanguage::Czech,
            LinguaLanguage::Japanese,
            LinguaLanguage::Russian,
            LinguaLanguage::Arabic,
        ])
        .build()
    });
    detector.detect_language_of(text).map(|language| match language {
        LinguaLanguage::English => Language::En,
        LinguaLanguage::German => Language::De,
        LinguaLanguage::French => Language::Fr,
        LinguaLanguage::Spanish => Language::Es,
        LinguaLanguage::Italian => Language::It,
        LinguaLanguage::Chinese => Language::Zh,
        LinguaLanguage::Portuguese => Language::Pt,
        LinguaLanguage::Polish => Language::Pl,
        LinguaLanguage::Turkish => Language::Tr,
        LinguaLanguage::Dutch => Language::Nl,
        LinguaLanguage::Czech => Language::Cs,
        LinguaLanguage::Japanese => Language::Ja,
        LinguaLanguage::Russian => Language::Ru,
        LinguaLanguage::Arabic => Language::Ar,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
    use base64::Engine;
    use common::currency::domain::Currency;
    use common::price::domain::MonetaryAmount;
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
        shop.shopify_currency = Some(Currency::Eur);
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
        product_service.expect_upsert().return_once(move |cmds| {
            Box::pin(async move {
                assert_eq!(cmds.len(), 1);
                assert_eq!(cmds[0].shop_id, shop.shop_id);
                assert_eq!(cmds[0].shops_product_id.to_string(), "17");
                assert_eq!(cmds[0].state, Some(ProductState::Available));
                assert_eq!(
                    cmds[0].native_price.as_ref().map(|price| price.monetary_amount),
                    Some(MonetaryAmount::from(4269_u64))
                );
                vec![]
            })
        });

        let response = handle(lambda_event, &get_shop_service, &product_service)
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
        product_service.expect_upsert().return_once(move |cmds| {
            Box::pin(async move {
                assert_eq!(cmds[0].state, Some(ProductState::Removed));
                assert!(cmds[0].native_title.is_none());
                vec![]
            })
        });

        let response = handle(lambda_event, &get_shop_service, &product_service)
            .await
            .unwrap();
        assert_eq!(200, response.status_code);
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

        let err = handle(
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

        let err = handle(
            lambda_event,
            &get_shop_service,
            &MockCommandProductService::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(400, err.status);
    }
}
