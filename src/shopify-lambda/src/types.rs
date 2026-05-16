use common::currency::domain::Currency;
use common::domain::Domain;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{MonetaryAmount, Price};
use common::product_state::domain::ProductState;
use common::shops_product_id::ShopsProductId;
use product::core::description::Description;
use product::core::product_image::ProductImage;
use product::core::prohibited_content::ProhibitedContent;
use product::core::title::Title;
use product::service::product_command::UpsertProductCommand;
use serde::Deserialize;
use url::Url;

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
    pub currency: Option<Currency>,
    pub language: Language,
}

#[derive(Debug, thiserror::Error)]
pub enum ShopifyProductEventError {
    #[error("Missing product title")]
    MissingTitle,
    #[error("Missing product handle")]
    MissingHandle,
    #[error("Invalid product URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("Invalid Shopify price '{0}'")]
    InvalidPrice(String),
    #[error("Shop has no Shopify currency configured")]
    MissingCurrency,
}

impl TryFrom<ShopifyProductEvent> for UpsertProductCommand {
    type Error = ShopifyProductEventError;

    fn try_from(event: ShopifyProductEvent) -> Result<Self, Self::Error> {
        let title = event
            .payload
            .title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .ok_or(ShopifyProductEventError::MissingTitle)?;
        let description = event
            .payload
            .body_html
            .as_deref()
            .map(html_to_text)
            .filter(|description| !description.is_empty());
        let language = event.language;
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
            seller_name_raw: None,
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
                event.currency,
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

pub fn html_to_text(html: &str) -> String {
    html2text::from_read(html.as_bytes(), 120)
        .unwrap_or_else(|_| String::new())
        .trim()
        .to_owned()
}

pub fn product_state(payload: &ShopifyProductPayload) -> ProductState {
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

pub fn parse_price(
    price: Option<&str>,
    currency: Option<Currency>,
) -> Result<Option<Price>, ShopifyProductEventError> {
    let Some(price) = price.filter(|price| !price.trim().is_empty()) else {
        return Ok(None);
    };
    let currency = currency.ok_or(ShopifyProductEventError::MissingCurrency)?;
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
    Ok(Some(Price::new(
        MonetaryAmount::from(major * 100 + minor),
        currency,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::currency::domain::Currency;
    use common::domain::Domain;
    use common::price::domain::{MonetaryAmount, Price};
    use common::shop_id::ShopId;
    use fake::{Fake, Faker};
    use shop::core::partner_status::ShopPartnerStatus;
    use shop::core::shop::Shop;

    fn shopify_detail(topic: &str) -> serde_json::Value {
        serde_json::json!({
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

    fn partnered_shop() -> Shop {
        let mut shop: Shop = Faker.fake();
        shop.shop_id = ShopId::new();
        shop.shopify_domain = Some(Domain::try_from("partner-shop.myshopify.com").unwrap());
        shop.shopify_currency = Some(Currency::Usd);
        shop.shopify_language = Some(Language::De);
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
            currency: shop.shopify_currency,
            language: shop.shopify_language.unwrap_or(Language::En),
            payload: serde_json::from_value(shopify_detail("products/update")["payload"].clone())
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
        assert_eq!(
            actual.native_title.map(|t| t.localization),
            Some(Language::De)
        );
    }

    #[test]
    fn should_map_delete_event_to_removed_state_for_shopify_product() {
        let shop = partnered_shop();
        let event = ShopifyProductEvent {
            shop_id: shop.shop_id,
            shop_domain: shop.shopify_domain.clone().unwrap(),
            kind: ShopifyProductEventKind::Delete,
            currency: shop.shopify_currency,
            language: shop.shopify_language.unwrap_or(Language::En),
            payload: serde_json::from_value(shopify_detail("products/delete")["payload"].clone())
                .unwrap(),
        };

        let actual = UpsertProductCommand::try_from(event).unwrap();

        assert_eq!(actual.state, Some(ProductState::Removed));
    }

    #[test]
    fn should_convert_html_description_to_text_when_html_is_malformed_for_html_to_text() {
        let actual = html_to_text("<p>Hello <strong>World");

        assert!(actual.contains("Hello"));
        assert!(actual.contains("World"));
    }
}
