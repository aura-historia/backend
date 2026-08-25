use indexmap::IndexSet;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_service::use_cases::IngestShopifyProductListingCommand;
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
    ) -> Result<IngestShopifyProductListingCommand, ShopifyProductEventError> {
        let availability = match self {
            Self::Delete => None,
            Self::Create | Self::Update => product_availability(&payload),
        };
        let title = payload
            .title
            .filter(|value| !value.trim().is_empty())
            .ok_or(ShopifyProductEventError::MissingTitle)?;
        let handle = payload
            .handle
            .filter(|value| !value.trim().is_empty())
            .ok_or(ShopifyProductEventError::MissingHandle)?;

        Ok(IngestShopifyProductListingCommand {
            shop_domain,
            shop_listing_id: product_listing_core::shop_listing_id::ShopListingId::from(
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
            availability,
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

pub fn product_availability(payload: &ShopifyProductPayload) -> Option<ListingAvailability> {
    if payload.variants.iter().any(|variant| {
        variant
            .inventory_quantity
            .is_some_and(|quantity| quantity > 0)
    }) {
        Some(ListingAvailability::InStock)
    } else if payload
        .variants
        .iter()
        .any(|variant| variant.inventory_quantity == Some(0))
        && payload.variants.iter().all(|variant| {
            variant
                .inventory_quantity
                .is_some_and(|quantity| quantity >= 0)
        })
    {
        Some(ListingAvailability::OutOfStock)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_clear_availability_for_delete_product_command() {
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

        assert_eq!(None, command.availability);
        assert_eq!("42", command.shop_listing_id.to_string());
    }

    #[test]
    fn should_map_positive_inventory_to_in_stock() {
        assert_eq!(
            Some(ListingAvailability::InStock),
            product_availability(&payload_with_inventory(Some(1)))
        );
    }

    #[test]
    fn should_map_zero_inventory_to_out_of_stock() {
        assert_eq!(
            Some(ListingAvailability::OutOfStock),
            product_availability(&payload_with_inventory(Some(0)))
        );
    }

    #[test]
    fn should_clear_availability_when_inventory_is_missing() {
        assert_eq!(None, product_availability(&payload_with_inventory(None)));
    }

    fn payload_with_inventory(inventory_quantity: Option<i64>) -> ShopifyProductPayload {
        ShopifyProductPayload {
            id: 42,
            title: None,
            body_html: None,
            handle: None,
            status: Some("active".to_owned()),
            variants: vec![ShopifyVariantPayload {
                price: None,
                inventory_quantity,
            }],
            images: Vec::new(),
        }
    }
}
