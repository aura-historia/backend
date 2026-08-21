use indexmap::IndexSet;
use product_core::product_state::ProductState;
use product_service::use_cases::IngestShopifyProductCommand;
use serde::Deserialize;
use url::Url;

#[derive(Debug, Clone, Deserialize)]
pub struct ShopifyEventDetail {
    pub payload: ShopifyProductPayload,
    pub metadata: ShopifyEventMetadata,
}

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, thiserror::Error)]
pub enum ShopifyProductEventError {
    #[error("Shopify product title is missing")]
    MissingTitle,
    #[error("Shopify product handle is missing")]
    MissingHandle,
}

impl ShopifyProductEventKind {
    pub fn command(
        self,
        shop_domain: shop_core::domain::Domain,
        payload: ShopifyProductPayload,
    ) -> Result<IngestShopifyProductCommand, ShopifyProductEventError> {
        let state = match self {
            Self::Delete => ProductState::Removed,
            Self::Create | Self::Update => product_state(&payload),
        };
        let title = payload
            .title
            .filter(|value| !value.trim().is_empty())
            .ok_or(ShopifyProductEventError::MissingTitle)?;
        let handle = payload
            .handle
            .filter(|value| !value.trim().is_empty())
            .ok_or(ShopifyProductEventError::MissingHandle)?;

        Ok(IngestShopifyProductCommand {
            shop_domain,
            shops_product_id: product_core::shops_product_id::ShopsProductId::from(
                payload.id.to_string(),
            ),
            title,
            description: payload
                .body_html
                .as_deref()
                .map(fallbacked_html_to_markdown)
                .filter(|value| !value.is_empty()),
            handle,
            price: payload
                .variants
                .first()
                .and_then(|variant| variant.price.clone()),
            state,
            image_urls: payload
                .images
                .into_iter()
                .map(|image| image.src)
                .collect::<IndexSet<_>>(),
        })
    }
}

pub fn fallbacked_html_to_markdown(html: &str) -> String {
    match html_to_markdown_rs::convert(html, None) {
        Ok(result) => result.content.unwrap_or_else(|| html.to_owned()),
        Err(_) => html.to_owned(),
    }
}

pub fn product_state(payload: &ShopifyProductPayload) -> ProductState {
    match payload.status.as_deref() {
        Some("active")
            if payload
                .variants
                .iter()
                .any(|variant| variant.inventory_quantity.unwrap_or_default() > 0) =>
        {
            ProductState::Available
        }
        Some("active") => ProductState::Sold,
        Some("draft") => ProductState::Listed,
        Some("archived") => ProductState::Removed,
        _ => ProductState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_map_delete_to_removed_product_command() {
        let payload = ShopifyProductPayload {
            id: 42,
            title: Some("Cabinet".to_owned()),
            body_html: None,
            handle: Some("cabinet".to_owned()),
            status: Some("active".to_owned()),
            variants: Vec::new(),
            images: Vec::new(),
        };

        let command = ShopifyProductEventKind::Delete
            .command(
                shop_core::domain::Domain::try_from("partner.example")
                    .unwrap_or_else(|error| panic!("invalid domain: {error}")),
                payload,
            )
            .unwrap_or_else(|error| panic!("failed mapping payload: {error}"));

        assert_eq!(ProductState::Removed, command.state);
        assert_eq!("42", command.shops_product_id.to_string());
    }

    #[test]
    fn should_map_active_in_stock_product_to_available() {
        let state = product_state(&ShopifyProductPayload {
            id: 42,
            title: None,
            body_html: None,
            handle: None,
            status: Some("active".to_owned()),
            variants: vec![ShopifyVariantPayload {
                price: None,
                inventory_quantity: Some(1),
            }],
            images: Vec::new(),
        });

        assert_eq!(ProductState::Available, state);
    }
}
