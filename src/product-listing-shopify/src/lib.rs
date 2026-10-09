use listing_source_service::ports::ShopifySource;
use product_listing_normalization::{
    NormalizationContext, NormalizationInputError, ProductListingNormalizationContextV1,
    ProductListingNormalizationInput, ProductListingRawValues, ProductListingRawValuesPatch,
    ProductListingRawValuesPriceFormat, RawProductListingOperation, RawProductListingPayloadFormat,
    RawProductListingValues, SourcePayload,
};
use product_listing_service::ports::shopify_product_decoder::{
    ShopifyListingAction, ShopifyProductDecoder, ShopifyProductEventKind, ShopifyRawObservation,
};
use serde::Deserialize;
use serde_json::Value;
use time::OffsetDateTime;
#[cfg(test)]
use time::format_description::well_known::Rfc3339;

const PAYLOAD_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Deserialize)]
struct ShopifyProductPayload {
    pub id: u64,
    #[serde(default)]
    pub title: Option<String>,

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
struct ShopifyVariantPayload {
    #[serde(default)]
    pub price: Option<String>,
    #[serde(default)]
    pub inventory_quantity: Option<i64>,
    #[serde(default)]
    pub inventory_management: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ShopifyImagePayload {
    pub src: String,
}

#[derive(Debug, thiserror::Error)]
enum ShopifyProductEventError {
    #[error("Shopify product payload is malformed")]
    MalformedPayload(#[source] serde_json::Error),
    #[error("Shopify product source payload is invalid")]
    InvalidSourcePayload(#[source] NormalizationInputError),
    #[error("Shopify product title is missing")]
    MissingTitle,
    #[error("Shopify product handle is missing")]
    MissingHandle,
    #[error("Shopify listing source currency is missing for a nonblank product price")]
    MissingListingSourceCurrency,
    #[cfg(test)]
    #[error("Shopify trigger timestamp is invalid")]
    InvalidTriggeredAt(#[source] time::error::Parse),
}

/// Maps Shopify's provider vocabulary to Aura's generic raw-input contract.
/// Unknown keys other than `body_html` remain in `source_payload` unchanged.
fn listing_action_with_source_occurred_at(
    kind: ShopifyProductEventKind,
    source: &ShopifySource,
    payload: Value,
    source_occurred_at: Option<OffsetDateTime>,
) -> Result<ShopifyListingAction, ShopifyProductEventError> {
    let mut sanitized_payload = payload;
    if let Some(object) = sanitized_payload.as_object_mut() {
        object.remove("body_html");
    }
    let source_payload = SourcePayload::new(sanitized_payload.clone())
        .map_err(ShopifyProductEventError::InvalidSourcePayload)?;
    let product = serde_json::from_value::<ShopifyProductPayload>(sanitized_payload)
        .map_err(ShopifyProductEventError::MalformedPayload)?;
    let source_record_key = product.id.to_string();

    let operation = if kind == ShopifyProductEventKind::Delete {
        Some(RawProductListingOperation::Delete)
    } else {
        match product.status.as_deref() {
            Some("active") => Some(RawProductListingOperation::Upsert),
            Some("archived" | "draft") => Some(RawProductListingOperation::Delete),
            Some(_) | None => None,
        }
    };
    let Some(operation) = operation else {
        return Ok(ShopifyListingAction::Ignore);
    };

    let raw_values = match operation {
        RawProductListingOperation::Upsert => active_raw_values(source, &product)?,
        RawProductListingOperation::Delete => {
            RawProductListingValues::new(serde_json::json!({}))
                .map_err(ShopifyProductEventError::InvalidSourcePayload)?
        }
    };
    let context = normalization_context(source)?;
    let input = ProductListingNormalizationInput::new(
        operation,
        RawProductListingPayloadFormat::ShopifyProduct,
        PAYLOAD_SCHEMA_VERSION,
        product_listing_normalization::PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
        source_payload,
        raw_values,
        context,
    )
    .map_err(ShopifyProductEventError::InvalidSourcePayload)?;
    Ok(ShopifyListingAction::Capture(ShopifyRawObservation {
        source_record_key,
        input,
        source_occurred_at,
    }))
}

#[cfg(test)]
fn source_occurred_at_from_triggered_at(
    triggered_at: Option<&str>,
) -> Result<Option<OffsetDateTime>, ShopifyProductEventError> {
    triggered_at
        .map(|value| {
            OffsetDateTime::parse(value, &Rfc3339)
                .map_err(ShopifyProductEventError::InvalidTriggeredAt)
        })
        .transpose()
}

fn active_raw_values(
    source: &ShopifySource,
    product: &ShopifyProductPayload,
) -> Result<RawProductListingValues, ShopifyProductEventError> {
    let title = product
        .title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or(ShopifyProductEventError::MissingTitle)?;
    let handle = product
        .handle
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or(ShopifyProductEventError::MissingHandle)?;
    let price = patch(
        product
            .variants
            .first()
            .and_then(|variant| variant.price.clone()),
    );
    if source.currency.is_none() && matches!(&price, ProductListingRawValuesPatch::Set(_)) {
        return Err(ShopifyProductEventError::MissingListingSourceCurrency);
    }
    let raw_values = ProductListingRawValues {
        source_listing_id: product.id.to_string(),
        title: ProductListingRawValuesPatch::Set(title.to_owned()),
        description: ProductListingRawValuesPatch::Unchanged,
        price_format: ProductListingRawValuesPriceFormat::MachineDecimal,
        price,
        price_estimate_min: ProductListingRawValuesPatch::Unchanged,
        price_estimate_max: ProductListingRawValuesPatch::Unchanged,
        availability: product_availability(product),
        url: ProductListingRawValuesPatch::Set(format!(
            "https://{}/products/{handle}",
            source.domain
        )),
        images: ProductListingRawValuesPatch::Set(
            product
                .images
                .iter()
                .map(|image| image.src.clone())
                .collect(),
        ),
        attributes: Default::default(),
    };
    serde_json::to_value(raw_values)
        .map_err(NormalizationInputError::JsonSerialization)
        .and_then(RawProductListingValues::new)
        .map_err(ShopifyProductEventError::InvalidSourcePayload)
}

fn normalization_context(
    source: &ShopifySource,
) -> Result<NormalizationContext, ShopifyProductEventError> {
    let context = ProductListingNormalizationContextV1 {
        base_url: format!("https://{}/", source.domain),
        fallback_currency: source.currency.map(|currency| currency.as_str().to_owned()),
        fallback_language: source.language.map(|language| language.as_str().to_owned()),
    };
    serde_json::to_value(context)
        .map_err(NormalizationInputError::JsonSerialization)
        .and_then(NormalizationContext::new)
        .map_err(ShopifyProductEventError::InvalidSourcePayload)
}

fn patch(value: Option<String>) -> ProductListingRawValuesPatch<String> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(ProductListingRawValuesPatch::Set)
        .unwrap_or(ProductListingRawValuesPatch::Clear)
}

/// Maps only reliable Shopify inventory facts. Missing and untracked inventory
/// explicitly clear Aura's current availability assertion.
fn product_availability(payload: &ShopifyProductPayload) -> ProductListingRawValuesPatch<String> {
    let quantities: Vec<i64> = payload
        .variants
        .iter()
        .filter_map(|variant| {
            variant
                .inventory_management
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .and(variant.inventory_quantity)
        })
        .collect();

    if quantities.iter().any(|quantity| *quantity > 0) {
        ProductListingRawValuesPatch::Set("in stock".to_owned())
    } else if !quantities.is_empty() {
        ProductListingRawValuesPatch::Set("out of stock".to_owned())
    } else {
        ProductListingRawValuesPatch::Clear
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use listing_source_core::{Domain, ListingSourceId};
    use localization::Language;
    use money::Currency;
    use serde_json::json;

    #[test]
    fn should_map_active_product_to_generic_raw_values_and_context() {
        let action = listing_action(ShopifyProductEventKind::Create,
                &source(),
                json!({
                    "id": 42,
                    "title": "Cabinet",
                    "body_html": "<p>Imported cabinet</p>",
                    "handle": "cabinet",
                    "status": "active",
                    "variants": [{"price": "42.00", "inventory_quantity": 1, "inventory_management": "shopify"}],
                    "images": [{"src": "https://images.example/cabinet.jpg"}],
                    "futureShopifyKey": {"nested": true}
                }),
            )
            .unwrap_or_else(|error| panic!("mapping failed: {error}"));

        let ShopifyListingAction::Capture(observation) = action else {
            panic!("active product must capture");
        };
        assert_eq!(observation.source_record_key, "42");
        assert_eq!(
            observation.input.operation(),
            RawProductListingOperation::Upsert
        );
        assert_eq!(
            product_listing_normalization::PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            observation.input.raw_values_schema_version()
        );
        assert_eq!(
            observation.input.raw_values().value()["priceFormat"],
            json!("MACHINE_DECIMAL")
        );
        assert_eq!(
            observation.input.raw_values().value()["availability"],
            json!({"action": "SET", "value": "in stock"})
        );
        assert_eq!(
            observation.input.normalization_context().value()["fallbackCurrency"],
            json!("USD")
        );
        assert_eq!(
            observation.input.normalization_context().value()["fallbackLanguage"],
            json!("de")
        );
        assert_eq!(
            observation.input.source_payload().value()["futureShopifyKey"]["nested"],
            json!(true)
        );
        assert_eq!(
            None,
            observation.input.source_payload().value().get("body_html")
        );
        assert_eq!(
            json!({"action": "UNCHANGED"}),
            observation.input.raw_values().value()["description"]
        );

        let alternative = listing_action(ShopifyProductEventKind::Create,
                &source(),
                json!({
                    "id": 42,
                    "title": "Cabinet",
                    "body_html": "A different description must be ignored",
                    "handle": "cabinet",
                    "status": "active",
                    "variants": [{"price": "42.00", "inventory_quantity": 1, "inventory_management": "shopify"}],
                    "images": [{"src": "https://images.example/cabinet.jpg"}],
                    "futureShopifyKey": {"nested": true}
                }),
            )
            .unwrap_or_else(|error| panic!("mapping failed: {error}"));
        let ShopifyListingAction::Capture(alternative) = alternative else {
            panic!("active product must capture");
        };
        assert_eq!(
            observation.input.raw_values().value(),
            alternative.input.raw_values().value()
        );
        assert_eq!(
            observation.input.source_payload().value(),
            alternative.input.source_payload().value()
        );

        let update = listing_action(
            ShopifyProductEventKind::Update,
            &source(),
            json!({
                "id": 42,
                "title": "Cabinet",
                "body_html": "An update description must also be ignored",
                "handle": "cabinet",
                "status": "active",
                "variants": [],
                "images": []
            }),
        )
        .unwrap_or_else(|error| panic!("mapping failed: {error}"));
        let ShopifyListingAction::Capture(update) = update else {
            panic!("active update must capture");
        };
        assert_eq!(
            json!({"action": "UNCHANGED"}),
            update.input.raw_values().value()["description"]
        );
        assert_eq!(None, update.input.source_payload().value().get("body_html"));
    }

    #[test]
    fn should_map_archived_draft_and_delete_to_raw_delete() {
        for (kind, status) in [
            (ShopifyProductEventKind::Delete, Some("active")),
            (ShopifyProductEventKind::Update, Some("archived")),
            (ShopifyProductEventKind::Update, Some("draft")),
        ] {
            let action = listing_action(kind, &source(), payload(status))
                .unwrap_or_else(|error| panic!("mapping failed: {error}"));
            assert!(matches!(
                action,
                ShopifyListingAction::Capture(ShopifyRawObservation { input, .. })
                    if input.operation() == RawProductListingOperation::Delete
                        && input.raw_values_schema_version()
                            == product_listing_normalization::PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION
            ));
        }
    }

    #[test]
    fn should_ignore_missing_or_unsupported_status_with_invalid_updated_at() {
        for status in [None, Some("published")] {
            let mut ignored_payload = payload(status);
            ignored_payload["updated_at"] = json!("not-a-timestamp");

            assert!(matches!(
                listing_action(ShopifyProductEventKind::Update, &source(), ignored_payload),
                Ok(ShopifyListingAction::Ignore)
            ));
        }
    }

    #[test]
    fn should_reject_nonblank_provider_price_when_listing_source_currency_is_missing() {
        let mut product = payload(Some("active"));
        product["variants"] = json!([{"price": "42.00"}]);

        assert!(matches!(
            listing_action(
                ShopifyProductEventKind::Create,
                &source_without_currency(),
                product
            ),
            Err(ShopifyProductEventError::MissingListingSourceCurrency)
        ));
    }

    #[test]
    fn should_capture_blank_provider_price_as_clear_when_listing_source_currency_is_missing() {
        let mut product = payload(Some("active"));
        product["variants"] = json!([{"price": " \t "}]);

        let action = listing_action(
            ShopifyProductEventKind::Create,
            &source_without_currency(),
            product,
        )
        .unwrap_or_else(|error| panic!("mapping failed: {error}"));

        let ShopifyListingAction::Capture(observation) = action else {
            panic!("blank-price product must capture");
        };
        assert_eq!(
            json!({"action": "CLEAR"}),
            observation.input.raw_values().value()["price"]
        );
    }

    #[test]
    fn should_parse_shopify_trigger_time_for_id_only_delete() {
        assert_eq!(
            Some(
                OffsetDateTime::parse("2026-09-07T10:02:00Z", &Rfc3339)
                    .unwrap_or_else(|error| panic!("timestamp: {error}")),
            ),
            source_occurred_at_from_triggered_at(Some("2026-09-07T10:02:00Z"))
                .unwrap_or_else(|error| panic!("trigger timestamp: {error}")),
        );
        assert!(matches!(
            listing_action_with_source_occurred_at(ShopifyProductEventKind::Delete,
                    &source(),
                    json!({"id": 42}),
                    source_occurred_at_from_triggered_at(Some("2026-09-07T10:02:00Z"))
                        .unwrap_or_else(|error| panic!("trigger timestamp: {error}")),
                ),
            Ok(ShopifyListingAction::Capture(ShopifyRawObservation {
                source_occurred_at: Some(_),
                input,
                ..
            })) if input.operation() == RawProductListingOperation::Delete
        ));
    }

    #[test]
    fn should_map_inventory_to_explicit_generic_intent() {
        assert_eq!(
            ProductListingRawValuesPatch::Set("in stock".to_owned()),
            product_availability(&payload_with_inventory(Some(1), Some("shopify")))
        );
        assert_eq!(
            ProductListingRawValuesPatch::Set("out of stock".to_owned()),
            product_availability(&payload_with_inventory(Some(0), Some("shopify")))
        );
        assert_eq!(
            ProductListingRawValuesPatch::Clear,
            product_availability(&payload_with_inventory(None, Some("shopify")))
        );
        assert_eq!(
            ProductListingRawValuesPatch::Clear,
            product_availability(&payload_with_inventory(Some(1), None))
        );
    }

    fn source() -> ShopifySource {
        ShopifySource {
            listing_source_id: ListingSourceId::new(),
            domain: Domain::try_from("partner.example")
                .unwrap_or_else(|error| panic!("invalid domain: {error}")),
            currency: Some(Currency::Usd),
            language: Some(Language::De),
        }
    }

    fn source_without_currency() -> ShopifySource {
        let mut source = source();
        source.currency = None;
        source
    }

    fn payload(status: Option<&str>) -> Value {
        json!({
            "id": 42,
            "title": "Cabinet",
            "handle": "cabinet",
            "status": status,
            "variants": [],
            "images": []
        })
    }

    fn payload_with_inventory(
        inventory_quantity: Option<i64>,
        inventory_management: Option<&str>,
    ) -> ShopifyProductPayload {
        ShopifyProductPayload {
            id: 42,
            title: Some("Cabinet".to_owned()),
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

#[derive(Clone, Copy, Default)]
pub struct ShopifyProductPayloadDecoder;

impl ShopifyProductDecoder for ShopifyProductPayloadDecoder {
    fn decode(
        &self,
        kind: ShopifyProductEventKind,
        source: &ShopifySource,
        payload: SourcePayload,
    ) -> Result<ShopifyListingAction, application::error::BoxError> {
        listing_action_with_source_occurred_at(kind, source, payload.value().clone(), None)
            .map_err(application::error::box_error)
    }
}

#[cfg(test)]
fn listing_action(
    kind: ShopifyProductEventKind,
    source: &ShopifySource,
    payload: Value,
) -> Result<ShopifyListingAction, ShopifyProductEventError> {
    listing_action_with_source_occurred_at(kind, source, payload, None)
}
