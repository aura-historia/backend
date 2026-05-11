use aws_lambda_events::eventbridge::EventBridgeEvent;
use common::currency::domain::Currency;
use common::domain::Domain;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{MonetaryAmount, Price};
use common::product_state::domain::ProductState;
use common::shops_product_id::ShopsProductId;
use lambda_runtime::LambdaEvent;
use lingua::{Language as LinguaLanguage, LanguageDetector, LanguageDetectorBuilder};
use product::core::description::Description;
use product::core::product_image::ProductImage;
use product::core::prohibited_content::ProhibitedContent;
use product::core::title::Title;
use product::service::command_service::CommandProductService;
use product::service::product_command::UpsertProductCommand;
use serde::Deserialize;
use serde_json::Value;
use shop::core::partner_status::ShopPartnerStatus;
use shop::service::get_service::{GetShopError, GetShopService};
use std::sync::OnceLock;
use tracing::{error, warn};
use url::Url;

pub const SHOPIFY_TOPIC_PRODUCTS_CREATE: &str = "products/create";
pub const SHOPIFY_TOPIC_PRODUCTS_UPDATE: &str = "products/update";
pub const SHOPIFY_TOPIC_PRODUCTS_DELETE: &str = "products/delete";

#[derive(Debug, Clone, Deserialize)]
pub struct ShopifyEventDetail {
    pub payload: ShopifyProductPayload,
    pub metadata: ShopifyEventMetadata,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ShopifyEventMetadata {
    #[serde(rename = "X-Shopify-Topic")]
    pub topic: String,
    #[serde(rename = "X-Shopify-Shop-Domain")]
    pub shop_domain: String,
    #[serde(rename = "X-Shopify-Event-Id", default)]
    pub event_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShopifyProductPayload {
    pub id: u64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body_html: Option<String>,
    #[serde(default)]
    pub handle: Option<String>,
    #[serde(default)]
    pub vendor: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub variants: Vec<ShopifyVariantPayload>,
    #[serde(default)]
    pub images: Vec<ShopifyImagePayload>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShopifyVariantPayload {
    #[serde(default)]
    pub price: Option<String>,
    #[serde(default)]
    pub inventory_quantity: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShopifyImagePayload {
    pub src: Url,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShopifyProductEventKind {
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone)]
pub struct ShopifyProductEvent {
    pub shop_id: common::shop_id::ShopId,
    pub shop_domain: Domain,
    pub kind: ShopifyProductEventKind,
    pub payload: ShopifyProductPayload,
}

#[derive(Debug, thiserror::Error)]
pub enum ShopifyProductEventError {
    #[error("Missing product handle")]
    MissingHandle,
    #[error("Invalid product URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("Invalid Shopify price '{0}'")]
    InvalidPrice(String),
}

#[tracing::instrument(
    skip(event, shop_service, product_service),
    fields(
        requestId = %event.context.request_id,
        eventBridgeEventId = tracing::field::Empty,
        shopifyEventId = tracing::field::Empty,
        shopifyTopic = tracing::field::Empty,
        shopifyDomain = tracing::field::Empty,
    )
)]
pub async fn handler(
    event: LambdaEvent<EventBridgeEvent<Value>>,
    shop_service: &(impl GetShopService + Sync),
    product_service: &(impl CommandProductService + Sync),
) -> Result<(), lambda_runtime::Error> {
    let payload = event.payload;
    let span = tracing::Span::current();
    if let Some(event_bridge_event_id) = payload.id.as_deref() {
        span.record("eventBridgeEventId", event_bridge_event_id);
    }

    let detail = match serde_json::from_value::<ShopifyEventDetail>(payload.detail) {
        Ok(detail) => detail,
        Err(err) => {
            error!(error = %err, "Failed deserializing Shopify EventBridge detail.");
            return Ok(());
        }
    };

    if let Some(event_id) = detail.metadata.event_id.as_deref() {
        span.record("shopifyEventId", event_id);
    }
    span.record("shopifyTopic", detail.metadata.topic.as_str());
    span.record("shopifyDomain", detail.metadata.shop_domain.as_str());

    let kind = match detail.metadata.topic.as_str() {
        SHOPIFY_TOPIC_PRODUCTS_CREATE => ShopifyProductEventKind::Create,
        SHOPIFY_TOPIC_PRODUCTS_UPDATE => ShopifyProductEventKind::Update,
        SHOPIFY_TOPIC_PRODUCTS_DELETE => ShopifyProductEventKind::Delete,
        other => {
            warn!(shopifyTopic = %other, "Received unsupported Shopify topic, ignoring.");
            return Ok(());
        }
    };

    let shop_domain = match Domain::try_from(detail.metadata.shop_domain.as_str()) {
        Ok(domain) => domain,
        Err(err) => {
            warn!(error = %err, "Shopify event contains invalid shop domain, ignoring.");
            return Ok(());
        }
    };

    let shop = match shop_service.find_shop_by_shopify_domain(&shop_domain).await {
        Ok(shop) => shop,
        Err(GetShopError::ShopifyDomainNotFound(_)) => {
            warn!(shopifyDomain = %shop_domain, "Shopify event references unknown shop, ignoring.");
            return Ok(());
        }
        Err(err) => return Err(Box::new(err)),
    };

    if shop.partner_status != ShopPartnerStatus::Partnered {
        warn!(shopId = %shop.shop_id, shopifyDomain = %shop_domain, "Shopify event references non-partner shop, ignoring.");
        return Ok(());
    }

    let event = ShopifyProductEvent {
        shop_id: shop.shop_id,
        shop_domain,
        kind,
        payload: detail.payload,
    };
    let command = match UpsertProductCommand::try_from(event) {
        Ok(command) => command,
        Err(err) => {
            error!(error = %err, "Failed mapping Shopify product event.");
            return Ok(());
        }
    };

    let failures = product_service.upsert(vec![command]).await;
    if failures.is_empty() {
        Ok(())
    } else {
        Err("Failed upserting Shopify product event".into())
    }
}

impl TryFrom<ShopifyProductEvent> for UpsertProductCommand {
    type Error = ShopifyProductEventError;

    fn try_from(event: ShopifyProductEvent) -> Result<Self, Self::Error> {
        let title = event.payload.title.clone().unwrap_or_default();
        let description = event
            .payload
            .body_html
            .as_deref()
            .map(html_to_text)
            .filter(|description| !description.is_empty());
        let language = infer_language(description.as_deref(), Some(title.as_str()));
        let handle = event
            .payload
            .handle
            .clone()
            .filter(|handle| !handle.is_empty())
            .ok_or(ShopifyProductEventError::MissingHandle)?;
        let url = Url::parse(&format!("https://{}/products/{handle}", event.shop_domain))?;
        let state = match event.kind {
            ShopifyProductEventKind::Delete => ProductState::Removed,
            ShopifyProductEventKind::Create | ShopifyProductEventKind::Update => {
                product_state(&event.payload)
            }
        };

        Ok(UpsertProductCommand {
            shop_id: event.shop_id,
            shops_product_id: ShopsProductId::from(event.payload.id.to_string()),
            seller_name_raw: event.payload.vendor.clone(),
            structured_address: None,
            geo_address: None,
            native_title: Some(Localized::new(language, Title::from(title))),
            native_description: description
                .map(Description::from)
                .map(|description| Localized::new(language, description)),
            native_price: parse_price(
                event
                    .payload
                    .variants
                    .first()
                    .and_then(|v| v.price.as_deref()),
            )?,
            native_price_estimate_min: None,
            native_price_estimate_max: None,
            state: Some(state),
            url: Some(url),
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

fn html_to_text(html: &str) -> String {
    html2text::from_read(html.as_bytes(), 120)
        .unwrap_or_else(|_| String::new())
        .trim()
        .to_owned()
}

fn product_state(payload: &ShopifyProductPayload) -> ProductState {
    match payload.status.as_deref() {
        Some("active") => {
            if payload
                .variants
                .iter()
                .any(|variant| variant.inventory_quantity.unwrap_or_default() > 0)
            {
                ProductState::Available
            } else {
                ProductState::Sold
            }
        }
        Some("draft") => ProductState::Listed,
        Some("archived") => ProductState::Removed,
        _ => ProductState::Unknown,
    }
}

fn parse_price(price: Option<&str>) -> Result<Option<Price>, ShopifyProductEventError> {
    let Some(price) = price.filter(|price| !price.trim().is_empty()) else {
        return Ok(None);
    };
    let trimmed = price.trim();
    let (major, minor) = trimmed.split_once('.').unwrap_or((trimmed, ""));
    if !major.chars().all(|c| c.is_ascii_digit()) || !minor.chars().all(|c| c.is_ascii_digit()) {
        return Err(ShopifyProductEventError::InvalidPrice(trimmed.to_owned()));
    }
    let major: u64 = major
        .parse()
        .map_err(|_| ShopifyProductEventError::InvalidPrice(trimmed.to_owned()))?;
    let mut minor = minor.chars().take(2).collect::<String>();
    while minor.len() < 2 {
        minor.push('0');
    }
    let minor: u64 = minor
        .parse()
        .map_err(|_| ShopifyProductEventError::InvalidPrice(trimmed.to_owned()))?;
    // TODO: Retrieve the Shopify shop currency instead of assuming USD.
    Ok(Some(Price::new(
        MonetaryAmount::from(major * 100 + minor),
        Currency::Usd,
    )))
}

fn infer_language(description: Option<&str>, title: Option<&str>) -> Language {
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
    detector
        .detect_language_of(text)
        .map(|language| match language {
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
    use aws_lambda_events::eventbridge::EventBridgeEvent;
    use common::price::domain::MonetaryAmount;
    use common::shop_id::ShopId;
    use fake::{Fake, Faker};
    use lambda_runtime::Context;
    use product::service::command_service::MockCommandProductService;
    use serde_json::json;
    use shop::core::shop::Shop;
    use shop::service::get_service::MockGetShopService;

    fn shopify_detail(topic: &str) -> Value {
        json!({
            "payload": {
                "id": 10231453024539_u64,
                "body_html": "<p>Hallo Test Beschreibung!</p>",
                "handle": "thomas-testprodukt",
                "title": "Thomas Testprodukt",
                "vendor": "partner vendor",
                "status": "active",
                "variants": [{"price": "420.00", "inventory_quantity": 2}],
                "images": [{"src": "https://cdn.shopify.com/product.jpg"}]
            },
            "metadata": {
                "X-Shopify-Topic": topic,
                "X-Shopify-Shop-Domain": "partner-shop.myshopify.com",
                "X-Shopify-Event-Id": "event-1"
            }
        })
    }

    fn lambda_event(topic: &str) -> LambdaEvent<EventBridgeEvent<Value>> {
        let mut event = EventBridgeEvent::<Value>::default();
        event.detail_type = "shopifyWebhook".to_string();
        event.source = "aws.partner/shopify.com/test".to_string();
        event.detail = shopify_detail(topic);
        LambdaEvent::new(event, Context::default())
    }

    fn partnered_shop() -> Shop {
        let mut shop: Shop = Faker.fake();
        shop.shop_id = ShopId::new();
        shop.shopify_domain = Some(Domain::try_from("partner-shop.myshopify.com").unwrap());
        shop.partner_status = ShopPartnerStatus::Partnered;
        shop
    }

    #[test]
    fn should_map_update_event_to_upsert_command_for_shopify_product() {
        let shop = partnered_shop();
        let event = ShopifyProductEvent {
            shop_id: shop.shop_id,
            shop_domain: shop.shopify_domain.clone().unwrap(),
            kind: ShopifyProductEventKind::Update,
            payload: serde_json::from_value(
                shopify_detail(SHOPIFY_TOPIC_PRODUCTS_UPDATE)["payload"].clone(),
            )
            .unwrap(),
        };

        let actual = UpsertProductCommand::try_from(event).unwrap();

        assert_eq!(actual.shop_id, shop.shop_id);
        assert_eq!(actual.shops_product_id.to_string(), "10231453024539");
        assert_eq!(actual.state, Some(ProductState::Available));
        assert_eq!(
            actual.native_price.unwrap(),
            Price::new(MonetaryAmount::from(42_000_u64), Currency::Usd)
        );
        assert_eq!(
            actual.url.unwrap().as_str(),
            "https://partner-shop.myshopify.com/products/thomas-testprodukt"
        );
        assert_eq!(actual.images.len(), 1);
    }

    #[test]
    fn should_map_delete_event_to_removed_state_for_shopify_product() {
        let shop = partnered_shop();
        let event = ShopifyProductEvent {
            shop_id: shop.shop_id,
            shop_domain: shop.shopify_domain.clone().unwrap(),
            kind: ShopifyProductEventKind::Delete,
            payload: serde_json::from_value(
                shopify_detail(SHOPIFY_TOPIC_PRODUCTS_DELETE)["payload"].clone(),
            )
            .unwrap(),
        };

        let actual = UpsertProductCommand::try_from(event).unwrap();

        assert_eq!(actual.state, Some(ProductState::Removed));
    }

    #[tokio::test]
    async fn should_upsert_product_when_shopify_event_is_valid_for_handler() {
        let shop = partnered_shop();
        let mut shop_service = MockGetShopService::default();
        shop_service
            .expect_find_shop_by_shopify_domain()
            .return_once(move |_| Box::pin(async move { Ok(shop) }));
        let mut product_service = MockCommandProductService::default();
        product_service.expect_upsert().return_once(|cmds| {
            Box::pin(async move {
                assert_eq!(cmds.len(), 1);
                vec![]
            })
        });

        let actual = handler(
            lambda_event(SHOPIFY_TOPIC_PRODUCTS_UPDATE),
            &shop_service,
            &product_service,
        )
        .await;

        assert!(actual.is_ok());
    }

    #[tokio::test]
    async fn should_skip_event_when_shop_is_not_partner_for_handler() {
        let mut shop = partnered_shop();
        shop.partner_status = ShopPartnerStatus::Scraped;
        let mut shop_service = MockGetShopService::default();
        shop_service
            .expect_find_shop_by_shopify_domain()
            .return_once(move |_| Box::pin(async move { Ok(shop) }));
        let mut product_service = MockCommandProductService::default();
        product_service.expect_upsert().never();

        let actual = handler(
            lambda_event(SHOPIFY_TOPIC_PRODUCTS_UPDATE),
            &shop_service,
            &product_service,
        )
        .await;

        assert!(actual.is_ok());
    }

    #[tokio::test]
    async fn should_return_error_when_product_upsert_fails_for_handler() {
        let shop = partnered_shop();
        let mut shop_service = MockGetShopService::default();
        shop_service
            .expect_find_shop_by_shopify_domain()
            .return_once(move |_| Box::pin(async move { Ok(shop) }));
        let mut product_service = MockCommandProductService::default();
        product_service
            .expect_upsert()
            .return_once(|cmds| Box::pin(async move { cmds }));

        let actual = handler(
            lambda_event(SHOPIFY_TOPIC_PRODUCTS_UPDATE),
            &shop_service,
            &product_service,
        )
        .await;

        assert!(actual.is_err());
    }
}
