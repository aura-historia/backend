use application::patch_field::PatchField;
use indexmap::IndexSet;
use listing_source_core::Domain;
use product_listing_core::{
    listing_availability::ListingAvailability, source_listing_id::SourceListingId,
};
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
    #[serde(default)]
    pub inventory_management: Option<String>,
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

#[derive(Debug)]
pub enum ShopifyListingAction {
    Ingest(Box<IngestShopifyProductListingCommand>),
    Withdraw,
    Ignore,
}

#[derive(Debug, thiserror::Error)]
pub enum ShopifyProductEventError {
    #[error("Shopify product source listing ID is invalid")]
    InvalidSourceListingId(
        #[source] product_listing_core::source_listing_id::InvalidSourceListingId,
    ),
    #[error("Shopify product title is missing")]
    MissingTitle,
    #[error("Shopify product handle is missing")]
    MissingHandle,
}

impl ShopifyProductEventKind {
    pub fn listing_action(
        self,
        source_domain: Domain,
        payload: ShopifyProductPayload,
    ) -> Result<ShopifyListingAction, ShopifyProductEventError> {
        if self == Self::Delete {
            return Ok(ShopifyListingAction::Withdraw);
        }

        match payload.status.as_deref() {
            Some("archived" | "draft") => Ok(ShopifyListingAction::Withdraw),
            Some("active") => Self::active_command(source_domain, payload)
                .map(Box::new)
                .map(ShopifyListingAction::Ingest),
            Some(_) | None => Ok(ShopifyListingAction::Ignore),
        }
    }

    fn active_command(
        source_domain: Domain,
        payload: ShopifyProductPayload,
    ) -> Result<IngestShopifyProductListingCommand, ShopifyProductEventError> {
        let availability = product_availability(&payload);
        let title = payload
            .title
            .filter(|value| !value.trim().is_empty())
            .ok_or(ShopifyProductEventError::MissingTitle)?;
        let handle = payload
            .handle
            .filter(|value| !value.trim().is_empty())
            .ok_or(ShopifyProductEventError::MissingHandle)?;

        Ok(IngestShopifyProductListingCommand {
            source_domain,
            source_listing_id: SourceListingId::try_from(payload.id.to_string())
                .map_err(ShopifyProductEventError::InvalidSourceListingId)?,
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

/// Maps only reliable Shopify inventory facts. Missing and untracked inventory
/// explicitly clears Aura's current availability assertion.
pub fn product_availability(payload: &ShopifyProductPayload) -> PatchField<ListingAvailability> {
    let tracked_quantities = payload.variants.iter().filter_map(|variant| {
        variant
            .inventory_management
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .and(variant.inventory_quantity)
    });
    let quantities: Vec<i64> = tracked_quantities.collect();

    if quantities.iter().any(|quantity| *quantity > 0) {
        PatchField::Set(ListingAvailability::InStock)
    } else if !quantities.is_empty() {
        PatchField::Set(ListingAvailability::OutOfStock)
    } else {
        PatchField::Clear
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_map_active_tracked_positive_inventory_to_in_stock() {
        assert_eq!(
            PatchField::Set(ListingAvailability::InStock),
            product_availability(&payload_with_inventory(Some(1), Some("shopify")))
        );
    }

    #[test]
    fn should_map_active_tracked_non_positive_inventory_to_out_of_stock() {
        assert_eq!(
            PatchField::Set(ListingAvailability::OutOfStock),
            product_availability(&payload_with_inventory(Some(0), Some("shopify")))
        );
    }

    #[test]
    fn should_clear_availability_when_inventory_is_missing_or_untracked() {
        assert_eq!(
            PatchField::Clear,
            product_availability(&payload_with_inventory(None, Some("shopify")))
        );
        assert_eq!(
            PatchField::Clear,
            product_availability(&payload_with_inventory(Some(1), None))
        );
    }

    #[test]
    fn should_withdraw_for_delete_archived_and_draft() {
        let domain = domain();
        for (kind, status) in [
            (ShopifyProductEventKind::Delete, Some("active")),
            (ShopifyProductEventKind::Update, Some("archived")),
            (ShopifyProductEventKind::Update, Some("draft")),
        ] {
            let mut payload = payload_with_inventory(Some(1), Some("shopify"));
            payload.status = status.map(str::to_owned);
            assert!(matches!(
                kind.listing_action(domain.clone(), payload),
                Ok(ShopifyListingAction::Withdraw)
            ));
        }
    }

    #[test]
    fn should_ignore_missing_or_unsupported_status_without_requiring_listing_data() {
        let mut payload = payload_with_inventory(Some(1), Some("shopify"));
        payload.status = None;
        assert!(matches!(
            ShopifyProductEventKind::Update.listing_action(domain(), payload),
            Ok(ShopifyListingAction::Ignore)
        ));
    }

    #[test]
    fn should_create_active_command_with_clear_for_untracked_inventory() {
        let action = ShopifyProductEventKind::Create
            .listing_action(domain(), payload_with_inventory(Some(1), None))
            .unwrap_or_else(|error| panic!("mapping failed: {error}"));
        assert!(matches!(
            action,
            ShopifyListingAction::Ingest(command)
                if command.availability == PatchField::Clear
        ));
    }

    fn domain() -> Domain {
        Domain::try_from("partner.example")
            .unwrap_or_else(|error| panic!("invalid domain: {error}"))
    }

    fn payload_with_inventory(
        inventory_quantity: Option<i64>,
        inventory_management: Option<&str>,
    ) -> ShopifyProductPayload {
        ShopifyProductPayload {
            id: 42,
            title: Some("Cabinet".to_owned()),
            body_html: None,
            handle: Some("cabinet".to_owned()),
            status: Some("active".to_owned()),
            variants: vec![ShopifyVariantPayload {
                price: None,
                inventory_quantity,
                inventory_management: inventory_management.map(str::to_owned),
            }],
            images: Vec::new(),
        }
    }
}
