mod webhook_verifier;

pub use webhook_verifier::{
    BlankShopifyClientSecret, ShopifyHmacWebhookVerifier, ShopifyWebhookSecrets,
};

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

const PAYLOAD_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Deserialize)]
struct ShopifyProductPayload {
    #[serde(deserialize_with = "deserialize_product_id")]
    pub product_id: u64,
    #[serde(default)]
    pub title: Option<String>,

    #[serde(default)]
    pub handle: Option<String>,
    #[serde(default)]
    pub variants: Vec<ShopifyVariantPayload>,
    #[serde(default)]
    pub images: Vec<ShopifyImagePayload>,
}

fn deserialize_product_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<u64, D::Error> {
    let id = u64::deserialize(deserializer)?;
    if id == 0 {
        return Err(serde::de::Error::custom("invalid Shopify product identity"));
    }
    Ok(id)
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
}

/// Maps Shopify's provider vocabulary to Aura's generic raw-input contract.
/// Unknown keys other than `body_html` remain in `source_payload` unchanged.
fn listing_action(
    kind: ShopifyProductEventKind,
    source: &ShopifySource,
    payload: Value,
) -> Result<ShopifyListingAction, ShopifyProductEventError> {
    let mut sanitized_payload = payload;
    if let Some(object) = sanitized_payload.as_object_mut() {
        object.remove("body_html");
    }
    let listing = sanitized_payload
        .get("product_listing")
        .cloned()
        .unwrap_or(Value::Null);
    let decoded_fields = if kind == ShopifyProductEventKind::Remove {
        serde_json::json!({"product_id": listing.get("product_id").cloned().unwrap_or(Value::Null)})
    } else {
        listing
    };
    let product = serde_json::from_value::<ShopifyProductPayload>(decoded_fields)
        .map_err(ShopifyProductEventError::MalformedPayload)?;
    if let Some(object) = sanitized_payload
        .get_mut("product_listing")
        .and_then(Value::as_object_mut)
    {
        object.remove("body_html");
    }
    let source_payload = SourcePayload::new(sanitized_payload)
        .map_err(ShopifyProductEventError::InvalidSourcePayload)?;
    let source_record_key = product.product_id.to_string();

    let operation = match kind {
        ShopifyProductEventKind::Add | ShopifyProductEventKind::Update => {
            RawProductListingOperation::Upsert
        }
        ShopifyProductEventKind::Remove => RawProductListingOperation::Delete,
    };

    let raw_values = match operation {
        RawProductListingOperation::Upsert => upsert_raw_values(source, &product)?,
        RawProductListingOperation::Delete => RawProductListingValues::new(
            serde_json::json!({"sourceListingId": product.product_id.to_string()}),
        )
        .map_err(ShopifyProductEventError::InvalidSourcePayload)?,
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
    }))
}

fn upsert_raw_values(
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
    let price = price_patch(
        product
            .variants
            .first()
            .and_then(|variant| variant.price.as_deref()),
    );
    if source.currency.is_none() && matches!(&price, ProductListingRawValuesPatch::Set(_)) {
        return Err(ShopifyProductEventError::MissingListingSourceCurrency);
    }
    let raw_values = ProductListingRawValues {
        source_listing_id: product.product_id.to_string(),
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

/// The provider's machine-decimal price without the whitespace around it, which the strict
/// machine-decimal reading would reject; a blank price clears it.
fn price_patch(value: Option<&str>) -> ProductListingRawValuesPatch<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| ProductListingRawValuesPatch::Set(value.to_owned()))
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

pub const SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD: &str = "product_listings/add";
pub const SHOPIFY_TOPIC_PRODUCT_LISTINGS_UPDATE: &str = "product_listings/update";
pub const SHOPIFY_TOPIC_PRODUCT_LISTINGS_REMOVE: &str = "product_listings/remove";

/// The catalog topics Aura forwards; every other topic, including broad `products/*`, is
/// acknowledged without forwarding.
pub fn product_listing_event_kind(topic: &str) -> Option<ShopifyProductEventKind> {
    match topic {
        SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD => Some(ShopifyProductEventKind::Add),
        SHOPIFY_TOPIC_PRODUCT_LISTINGS_UPDATE => Some(ShopifyProductEventKind::Update),
        SHOPIFY_TOPIC_PRODUCT_LISTINGS_REMOVE => Some(ShopifyProductEventKind::Remove),
        _ => None,
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
        listing_action(kind, source, payload.value().clone()).map_err(application::error::box_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use listing_source_core::{Domain, ListingSourceId};
    use localization::Language;
    use money::Currency;
    use serde_json::json;

    /// Decodes one webhook body whose `product_listing` is `listing`.
    fn wrapped_listing_action(
        kind: ShopifyProductEventKind,
        source: &ShopifySource,
        listing: Value,
    ) -> Result<ShopifyListingAction, ShopifyProductEventError> {
        listing_action(kind, source, json!({"product_listing": listing}))
    }

    #[test]
    fn should_map_active_product_to_generic_raw_values_and_context() {
        let action = wrapped_listing_action(ShopifyProductEventKind::Add,
                &source(),
                json!({
                    "product_id": 42,
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
        let values = observation.input.raw_values().value();
        assert_eq!(json!({"action":"SET","value":"Cabinet"}), values["title"]);
        assert_eq!(
            json!({"action":"SET","value":"https://partner.example/products/cabinet"}),
            values["url"]
        );
        assert_eq!(
            json!({"action":"SET","value":["https://images.example/cabinet.jpg"]}),
            values["images"]
        );
        assert_eq!(json!({"action":"SET","value":"42.00"}), values["price"]);
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
            observation.input.source_payload().value()["product_listing"]["futureShopifyKey"]["nested"],
            json!(true)
        );
        assert_eq!(
            None,
            observation.input.source_payload().value()["product_listing"].get("body_html")
        );
        assert_eq!(
            json!({"action": "UNCHANGED"}),
            observation.input.raw_values().value()["description"]
        );

        let alternative = wrapped_listing_action(ShopifyProductEventKind::Add,
                &source(),
                json!({
                    "product_id": 42,
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

        let update = wrapped_listing_action(
            ShopifyProductEventKind::Update,
            &source(),
            json!({
                "product_id": 42,
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
        assert_eq!(
            None,
            update.input.source_payload().value()["product_listing"].get("body_html")
        );
    }

    #[test]
    fn should_decode_listing_topics_without_product_status_and_reject_invalid_identity() {
        for kind in [
            ShopifyProductEventKind::Add,
            ShopifyProductEventKind::Update,
        ] {
            let mut snapshot = payload(None);
            snapshot.as_object_mut().unwrap().remove("status");
            assert!(matches!(
                wrapped_listing_action(kind, &source(), snapshot),
                Ok(ShopifyListingAction::Capture(_))
            ));
        }
        assert!(
            matches!(wrapped_listing_action(ShopifyProductEventKind::Remove, &source(), json!({"product_id": 42})), Ok(ShopifyListingAction::Capture(o)) if o.input.operation() == RawProductListingOperation::Delete)
        );
        for id in [
            json!(null),
            json!(0),
            json!(-1),
            json!(1.5),
            json!("42"),
            json!("invalid"),
            json!("042"),
        ] {
            assert!(
                wrapped_listing_action(
                    ShopifyProductEventKind::Remove,
                    &source(),
                    json!({"product_id": id})
                )
                .is_err()
            );
        }
        let exact = 9_007_199_254_740_993_u64;
        let action = wrapped_listing_action(
            ShopifyProductEventKind::Remove,
            &source(),
            json!({"product_id": exact}),
        )
        .unwrap();
        assert!(
            matches!(action, ShopifyListingAction::Capture(o) if o.source_record_key == exact.to_string())
        );
    }

    #[test]
    fn should_preserve_exact_unsigned_64_bit_product_ids() {
        for id in [9_007_199_254_740_993_u64, u64::MAX] {
            let ShopifyListingAction::Capture(observation) = wrapped_listing_action(
                ShopifyProductEventKind::Remove,
                &source(),
                json!({"product_id": id}),
            )
            .unwrap() else {
                panic!("must capture")
            };
            assert_eq!(id.to_string(), observation.source_record_key);
            assert_eq!(
                json!(id.to_string()),
                observation.input.raw_values().value()["sourceListingId"]
            );
        }
    }

    #[test]
    fn should_reject_nonblank_provider_price_when_listing_source_currency_is_missing() {
        let mut product = payload(Some("active"));
        product["variants"] = json!([{"price": "42.00"}]);

        assert!(matches!(
            wrapped_listing_action(
                ShopifyProductEventKind::Add,
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

        let action = wrapped_listing_action(
            ShopifyProductEventKind::Add,
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
    fn should_capture_the_price_without_surrounding_whitespace() {
        let mut product = payload(Some("active"));
        product["variants"] = json!([{"price": " 42.00\n"}]);

        let action = wrapped_listing_action(ShopifyProductEventKind::Add, &source(), product)
            .unwrap_or_else(|error| panic!("mapping failed: {error}"));

        let ShopifyListingAction::Capture(observation) = action else {
            panic!("priced product must capture");
        };
        assert_eq!(
            json!({"action": "SET", "value": "42.00"}),
            observation.input.raw_values().value()["price"]
        );
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
            "product_id": 42,
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
            product_id: 42,
            title: Some("Cabinet".to_owned()),
            handle: Some("cabinet".to_owned()),
            variants: vec![ShopifyVariantPayload {
                price: None,
                inventory_quantity,
                inventory_management: inventory_management.map(str::to_owned),
            }],
            images: Vec::new(),
        }
    }
}
