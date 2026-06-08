use common::language::data::LocalizedTextData;
use common::price::data::PriceData;
use common::price::domain::{MonetaryAmount, Price};
use common::product_state::domain::ProductState;
use common::shops_product_id::ShopsProductId;
use product::core::description::Description;
use product::core::title::Title;
use product::data::product_state_data::ProductStateData;
use product_lambda_ingest_partner_products::{
    AsyncProductCommandData, UpdateAsyncProductCommandData, UpsertAsyncProductCommandData,
};
use serde::Deserialize;
use shop::core::partner_shop::PartnerShop;
use tracing::warn;
use url::Url;

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
    #[error("Shop has no language configured")]
    MissingLanguage,
}

impl TryFrom<WoocommerceProductEvent> for AsyncProductCommandData {
    type Error = WoocommerceProductEventError;

    fn try_from(event: WoocommerceProductEvent) -> Result<Self, Self::Error> {
        let async_command_data = match event.kind {
            WoocommerceProductEventKind::Create | WoocommerceProductEventKind::Update => {
                let title = event
                    .payload
                    .name
                    .as_deref()
                    .filter(|title| !title.trim().is_empty())
                    .ok_or(WoocommerceProductEventError::MissingTitle)?;
                let description = event
                    .payload
                    .description
                    .as_deref()
                    .or(event.payload.short_description.as_deref())
                    .map(html_to_text)
                    .filter(|description| !description.is_empty());
                let language = event
                    .shop
                    .woocommerce_language
                    .ok_or(WoocommerceProductEventError::MissingLanguage)?;
                let state = product_state(&event.payload);
                let url = event
                    .payload
                    .permalink
                    .ok_or(WoocommerceProductEventError::MissingUrl)?;
                let images = if event.payload.images.is_empty() {
                    None
                } else {
                    Some(
                        event
                            .payload
                            .images
                            .into_iter()
                            .map(|image| image.src)
                            .collect(),
                    )
                };

                AsyncProductCommandData::Upsert(UpsertAsyncProductCommandData {
                    shop_id: event.shop.shop_id,
                    shops_product_id: ShopsProductId::from(event.payload.id.to_string()),
                    seller_name: None,
                    structured_address: None,
                    geo_address: None,
                    title: Some(LocalizedTextData::new(Title::from(title), language.into())),
                    description: description
                        .map(Description::from)
                        .map(|description| LocalizedTextData::new(description, language.into())),
                    price: parse_price(
                        event.payload.price.as_deref(),
                        event.shop.woocommerce_currency,
                    )?
                    .map(PriceData::from),
                    price_estimate_min: None,
                    price_estimate_max: None,
                    state: Some(state.into()),
                    url: Some(url),
                    images,
                    auction_start: None,
                    auction_end: None,
                })
            }
            WoocommerceProductEventKind::Delete => {
                AsyncProductCommandData::Update(UpdateAsyncProductCommandData {
                    shop_id: event.shop.shop_id,
                    shops_product_id: ShopsProductId::from(event.payload.id.to_string()),
                    price: None,
                    state: Some(ProductStateData::Removed),
                    price_estimate_min: None,
                    price_estimate_max: None,
                    url: None,
                    images: None,
                    auction_start: None,
                    auction_end: None,
                })
            }
        };

        Ok(async_command_data)
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
    let mut minor = if minor.is_empty() {
        "0".to_owned()
    } else {
        minor.chars().take(2).collect::<String>()
    };
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
