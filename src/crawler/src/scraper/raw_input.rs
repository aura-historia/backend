//! Crawler-owned mapping from selected CSS extraction output to the generic raw-input contract.

use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use money::Currency;
use product_listing_normalization::{
    NormalizationContext, NormalizationInputError, ProductListingNormalizationInput,
    ProductListingRawValuesPatch, ProductListingRawValuesV1, RawProductListingOperation,
    RawProductListingPayloadFormat, RawProductListingProvenance, RawProductListingValues,
    SourcePayload,
};
use serde_json::json;
use std::collections::BTreeMap;
use url::Url;

/// Builds the complete V1 input retained by the operational raw-revision stream.
///
/// This mapping intentionally retains source strings without applying typed field normalization.
/// The worker is the sole canonical normalization authority.
pub(crate) fn crawler_raw_input(
    raw: &RawExtractedProduct,
    candidate_url: &Url,
    default_currency: Option<Currency>,
) -> Result<ProductListingNormalizationInput, NormalizationInputError> {
    let source_payload = serde_json::to_value(raw)
        .map_err(NormalizationInputError::JsonSerialization)
        .and_then(SourcePayload::new)?;
    let attributes = raw
        .raw_attributes
        .iter()
        .map(|(key, values)| {
            (
                key.clone(),
                ProductListingRawValuesPatch::Set(values.clone()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let raw_values = ProductListingRawValuesV1 {
        source_listing_id: raw.source_listing_id.clone(),
        title: ProductListingRawValuesPatch::Set(raw.title.clone()),
        description: ProductListingRawValuesPatch::Set(raw.description.clone()),
        price: patch(raw.price.clone()),
        price_estimate_min: patch(raw.price_estimate_min.clone()),
        price_estimate_max: patch(raw.price_estimate_max.clone()),
        availability: ProductListingRawValuesPatch::Set(raw.state.clone()),
        url: ProductListingRawValuesPatch::Set(candidate_url.to_string()),
        images: ProductListingRawValuesPatch::Set(raw.images.clone()),
        auction_start: patch(raw.auction_start.clone()),
        auction_end: patch(raw.auction_end.clone()),
        attributes,
    };
    let raw_values = serde_json::to_value(raw_values)
        .map_err(NormalizationInputError::JsonSerialization)
        .and_then(RawProductListingValues::new)?;
    let context = NormalizationContext::new(json!({
        "baseUrl": candidate_url,
        "fallbackCurrency": default_currency.map(|currency| currency.as_str()),
    }))?;

    ProductListingNormalizationInput::new(
        RawProductListingOperation::Upsert,
        RawProductListingPayloadFormat::CrawlerExtractedProduct,
        1,
        1,
        source_payload,
        raw_values,
        context,
    )
}

/// Builds durable evidence for a crawler-verified removal without retaining HTML or response data.
pub(crate) fn crawler_verified_removal_input(
    candidate_url: &Url,
) -> Result<ProductListingNormalizationInput, NormalizationInputError> {
    ProductListingNormalizationInput::new(
        RawProductListingOperation::Delete,
        RawProductListingPayloadFormat::CrawlerExtractedProduct,
        1,
        1,
        SourcePayload::new(json!({
            "candidateUrl": candidate_url,
            "removalEvidence": "VERIFIED",
        }))?,
        RawProductListingValues::new(json!({}))?,
        NormalizationContext::new(json!({
            "baseUrl": candidate_url,
            "fallbackCurrency": null,
        }))?,
    )
}

/// Builds non-input crawler provenance. Page/schema data is excluded from the input hash.
pub(crate) fn crawler_provenance(
    page_hash: Option<&str>,
    schema_fingerprint: Option<&str>,
) -> Result<RawProductListingProvenance, NormalizationInputError> {
    RawProductListingProvenance::new(json!({
        "pageHash": page_hash,
        "schemaFingerprint": schema_fingerprint,
    }))
}

fn patch(value: Option<String>) -> ProductListingRawValuesPatch<String> {
    match value {
        Some(value) => ProductListingRawValuesPatch::Set(value),
        None => ProductListingRawValuesPatch::Clear,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw() -> RawExtractedProduct {
        RawExtractedProduct {
            source_listing_id: "SKU-1".to_owned(),
            title: "Chair".to_owned(),
            description: vec!["Oak".to_owned()],
            price: Some("100 EUR".to_owned()),
            price_estimate_min: None,
            price_estimate_max: None,
            state: "In Stock".to_owned(),
            images: vec!["/chair.jpg".to_owned()],
            auction_start: None,
            auction_end: None,
            raw_attributes: BTreeMap::new(),
        }
    }

    #[test]
    fn should_preserve_selected_raw_values_in_capture_input() -> Result<(), NormalizationInputError>
    {
        let url = Url::parse("https://example.com/products/1")
            .unwrap_or_else(|error| panic!("static test URL must parse: {error}"));
        let input = crawler_raw_input(&raw(), &url, Some(Currency::Eur))?;

        assert_eq!(
            Some(&serde_json::Value::String("100 EUR".to_owned())),
            input.source_payload().value().get("price")
        );
        assert_eq!(
            Some(&serde_json::json!({"action": "SET", "value": "100 EUR"})),
            input.raw_values().value().get("price")
        );
        assert_eq!(
            Some(&serde_json::json!(["/chair.jpg"])),
            input.source_payload().value().get("images")
        );
        Ok(())
    }

    #[test]
    fn should_hash_dynamic_raw_attribute_changes() -> Result<(), NormalizationInputError> {
        let url = Url::parse("https://example.com/products/1")
            .unwrap_or_else(|error| panic!("static test URL must parse: {error}"));
        let first = crawler_raw_input(&raw(), &url, Some(Currency::Eur))?.hash()?;
        let mut changed = raw();
        changed
            .raw_attributes
            .insert("rawMaterial".to_owned(), vec!["Oak".to_owned()]);
        let second = crawler_raw_input(&changed, &url, Some(Currency::Eur))?.hash()?;
        assert_ne!(first, second);
        Ok(())
    }
}
