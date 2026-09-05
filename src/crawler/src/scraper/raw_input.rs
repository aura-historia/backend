//! Crawler-owned mapping from selected CSS extraction output to the generic raw-input contract.

use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use money::Currency;
use product_listing_normalization::{
    NormalizationContext, NormalizationInputError, NormalizationInputHash,
    ProductListingNormalizationInput, ProductListingRawValuesPatch, ProductListingRawValuesV1,
    RawProductListingOperation, RawProductListingPayloadFormat, RawProductListingValues,
    SourcePayload,
};
use serde_json::json;
use std::collections::BTreeMap;
use url::Url;

pub(crate) fn crawler_raw_input_hash(
    raw: &RawExtractedProduct,
    candidate_url: &Url,
    default_currency: Option<Currency>,
) -> Result<NormalizationInputHash, NormalizationInputError> {
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
    )?
    .hash()
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
    fn should_hash_dynamic_raw_attribute_changes() -> Result<(), NormalizationInputError> {
        let url = Url::parse("https://example.com/products/1")
            .unwrap_or_else(|error| panic!("static test URL must parse: {error}"));
        let first = crawler_raw_input_hash(&raw(), &url, Some(Currency::Eur))?;
        let mut changed = raw();
        changed
            .raw_attributes
            .insert("rawMaterial".to_owned(), vec!["Oak".to_owned()]);
        let second = crawler_raw_input_hash(&changed, &url, Some(Currency::Eur))?;
        assert_ne!(first, second);
        Ok(())
    }
}
