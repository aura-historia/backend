//! Crawler-owned mapping from selected CSS extraction output to the generic raw-input contract.

use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use crate::scraper::plausibility::PlausibilityField;
use money::{Currency, Price};
use product_listing_core::source_listing_id::{InvalidSourceListingId, SourceListingId};
use product_listing_normalization::{
    NormalizationContext, NormalizationError, NormalizationInputError,
    ProductListingNormalizationInput, ProductListingRawValuesPatch,
    ProductListingRawValuesPriceFormat, RawProductListingOperation, RawProductListingPayloadFormat,
    RawProductListingProvenance, RawProductListingValues, SourcePayload, normalize_price,
    normalize_product_listing_price, normalize_source_listing_id_with_url_sha_fallback,
};
use serde_json::json;
use std::collections::BTreeMap;
use url::Url;

/// The source listing id a raw extraction is captured under: the extracted identity,
/// or the hash of the stable raw-stream key when the page yields none. A source
/// record key that is not a URL is invalid crawler state and fails closed.
pub(crate) fn capture_source_listing_id(
    raw_source_listing_id: &str,
    source_record_key: &str,
) -> Result<SourceListingId, NormalizationError> {
    let source_record_url = Url::parse(source_record_key)
        .map_err(|_| NormalizationError::SourceListingIdInvalid(InvalidSourceListingId::Blank))?;
    normalize_source_listing_id_with_url_sha_fallback(raw_source_listing_id, &source_record_url)
}

/// Builds the complete current raw input retained by the operational raw-revision stream.
///
/// Source payload keeps the untouched extraction. Generic raw values use the
/// crawler-validated image projection and keep every supplied price, state and text
/// value as extracted: the worker (the sole canonical normalization authority) parses
/// them, leaves a canonical field it cannot resolve unchanged and reports why. Only a
/// missing extraction clears a price. `carried_forward_title`, the title the listing was last
/// captured with, stands in for a title the page left blank; the source payload still shows the
/// blank one. A raw attribute whose rule found only a blank value is left out of the generic raw
/// values, which keep attributes as given; the source payload still shows it.
pub(crate) fn crawler_raw_input(
    raw: &RawExtractedProduct,
    validated_image_urls: &[String],
    candidate_url: &Url,
    source_listing_id: &str,
    fallback_currency: Option<Currency>,
    carried_forward_title: Option<&str>,
) -> Result<ProductListingNormalizationInput, NormalizationInputError> {
    let source_payload = serde_json::to_value(raw)
        .map_err(NormalizationInputError::JsonSerialization)
        .and_then(SourcePayload::new)?;
    let attributes = raw
        .raw_attributes
        .iter()
        .filter(|(_, values)| values.iter().any(|value| !value.trim().is_empty()))
        .map(|(key, values)| {
            (
                key.clone(),
                ProductListingRawValuesPatch::Set(values.clone()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let raw_values = RawProductListingValues::new(json!({
        "sourceListingId": source_listing_id,
        "title": ProductListingRawValuesPatch::Set(
            carried_forward_title.map_or_else(|| raw.title.clone(), str::to_owned)
        ),
        "description": ProductListingRawValuesPatch::Set(raw.description.clone()),
        "priceFormat": ProductListingRawValuesPriceFormat::DisplayText,
        "price": patch(raw.price.clone()),
        "priceEstimateMin": patch(raw.price_estimate_min.clone()),
        "priceEstimateMax": patch(raw.price_estimate_max.clone()),
        "availability": ProductListingRawValuesPatch::Set(raw.state.clone()),
        "url": ProductListingRawValuesPatch::Set(candidate_url.to_string()),
        "images": ProductListingRawValuesPatch::Set(validated_image_urls.to_vec()),
        "attributes": attributes,
    }))?;
    let context = NormalizationContext::new(json!({
        "baseUrl": candidate_url,
        "fallbackCurrency": fallback_currency.map(|currency| currency.as_str()),
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

/// A captured price that names another currency than its ListingSource's fallback currency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForeignPriceCurrency {
    pub(crate) field: PlausibilityField,
    pub(crate) currency: Currency,
    pub(crate) fallback_currency: Currency,
}

/// The first price of `raw`, in schema field order, whose currency, as the ProductListing
/// normalization worker reads it, is not `fallback_currency`. The main price goes through the
/// worker's own `normalize_product_listing_price`, the estimates through its `normalize_price`,
/// so the check cannot drift from what the worker persists. A storefront that converted its
/// prices for the crawler's egress country shows here and nowhere else, since the converted
/// price passes every plausibility check (`docs/crawler/networking.md#request-headers`). The
/// capture keeps the value as extracted either way.
pub(crate) fn foreign_price_currency(
    raw: &RawExtractedProduct,
    fallback_currency: Option<Currency>,
) -> Option<ForeignPriceCurrency> {
    let fallback_currency = fallback_currency?;
    let main_price = normalize_product_listing_price(raw.price.as_deref(), Some(fallback_currency))
        .ok()
        .flatten()
        .and_then(|price| price.monetary());
    [
        (PlausibilityField::Price, main_price),
        (
            PlausibilityField::PriceEstimateMin,
            estimate_price(raw.price_estimate_min.as_deref(), fallback_currency),
        ),
        (
            PlausibilityField::PriceEstimateMax,
            estimate_price(raw.price_estimate_max.as_deref(), fallback_currency),
        ),
    ]
    .into_iter()
    .find_map(|(field, price)| {
        let price = price.filter(|price| price.currency != fallback_currency)?;
        Some(ForeignPriceCurrency {
            field,
            currency: price.currency,
            fallback_currency,
        })
    })
}

fn estimate_price(value: Option<&str>, fallback_currency: Currency) -> Option<Price> {
    normalize_price(value, Some(fallback_currency))
        .ok()
        .flatten()
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
    use crate::scraper::css_selector::rule::IMAGE_CANDIDATE_SEPARATOR;

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
            raw_attributes: BTreeMap::new(),
        }
    }

    mod foreign_price_currency {
        use super::*;

        fn priced(
            price: Option<&str>,
            estimate_min: Option<&str>,
            estimate_max: Option<&str>,
        ) -> RawExtractedProduct {
            RawExtractedProduct {
                price: price.map(str::to_owned),
                price_estimate_min: estimate_min.map(str::to_owned),
                price_estimate_max: estimate_max.map(str::to_owned),
                ..raw()
            }
        }

        #[test]
        fn should_report_a_price_converted_to_another_currency() {
            let converted = priced(Some("$38,284.00"), None, None);

            assert_eq!(
                foreign_price_currency(&converted, Some(Currency::Gbp)),
                Some(ForeignPriceCurrency {
                    field: PlausibilityField::Price,
                    currency: Currency::Usd,
                    fallback_currency: Currency::Gbp,
                })
            );
        }

        #[test]
        fn should_read_the_currency_a_json_ld_offer_price_carries() {
            // A `json_ld_offer` price is captured with its offer's `priceCurrency`.
            assert_eq!(
                foreign_price_currency(
                    &priced(Some("1850.00 USD"), None, None),
                    Some(Currency::Gbp)
                ),
                Some(ForeignPriceCurrency {
                    field: PlausibilityField::Price,
                    currency: Currency::Usd,
                    fallback_currency: Currency::Gbp,
                })
            );
            assert_eq!(
                foreign_price_currency(
                    &priced(Some("1850.00 GBP"), None, None),
                    Some(Currency::Gbp)
                ),
                None
            );
        }

        #[test]
        fn should_report_the_first_estimate_in_another_currency_when_the_price_agrees() {
            let estimates = priced(Some("£1,000"), Some("£800"), Some("€1.200"));

            assert_eq!(
                foreign_price_currency(&estimates, Some(Currency::Gbp)),
                Some(ForeignPriceCurrency {
                    field: PlausibilityField::PriceEstimateMax,
                    currency: Currency::Eur,
                    fallback_currency: Currency::Gbp,
                })
            );
        }

        #[test]
        fn should_report_nothing_for_a_price_the_fallback_currency_covers() {
            for price in [
                Some("£28,000"),
                Some("28,000"),
                Some("POA"),
                Some(" "),
                None,
            ] {
                assert_eq!(
                    foreign_price_currency(&priced(price, None, None), Some(Currency::Gbp)),
                    None,
                    "{price:?}"
                );
            }
        }

        #[test]
        fn should_report_nothing_without_a_fallback_currency() {
            assert_eq!(
                foreign_price_currency(&priced(Some("$38,284.00"), None, None), None),
                None
            );
        }
    }

    #[test]
    fn should_preserve_selected_raw_values_in_capture_input() -> Result<(), NormalizationInputError>
    {
        let url = Url::parse("https://example.com/products/1")
            .unwrap_or_else(|error| panic!("static test URL must parse: {error}"));
        let extracted = raw();
        let input = crawler_raw_input(
            &extracted,
            &extracted.images,
            &url,
            "stable-id",
            Some(Currency::Eur),
            None,
        )?;

        assert_eq!(
            Some(&serde_json::Value::String("100 EUR".to_owned())),
            input.source_payload().value().get("price")
        );
        assert_eq!(
            Some(&serde_json::json!({"action": "SET", "value": "100 EUR"})),
            input.raw_values().value().get("price")
        );
        assert!(input.raw_values().value().get("auction").is_none());
        assert_eq!(
            Some(&serde_json::json!(["/chair.jpg"])),
            input.source_payload().value().get("images")
        );
        Ok(())
    }

    #[test]
    fn should_keep_unresolved_price_for_worker_preservation() -> Result<(), NormalizationInputError>
    {
        let url = Url::parse("https://example.com/products/1")
            .unwrap_or_else(|error| panic!("static test URL must parse: {error}"));
        let extracted = raw();

        let input =
            crawler_raw_input(&extracted, &extracted.images, &url, "stable-id", None, None)?;

        assert_eq!(
            Some(&serde_json::Value::String("100 EUR".to_owned())),
            input.source_payload().value().get("price")
        );
        assert_eq!(
            Some(&serde_json::json!({"action": "SET", "value": "100 EUR"})),
            input.raw_values().value().get("price")
        );
        assert_eq!(
            Some(&serde_json::json!({"action": "CLEAR"})),
            input.raw_values().value().get("priceEstimateMin")
        );
        Ok(())
    }

    #[test]
    fn should_capture_the_extracted_identity_when_present() {
        let identity = capture_source_listing_id(" SKU-1 ", "https://example.com/products/1")
            .unwrap_or_else(|error| panic!("extracted identity must be captured: {error}"));

        assert_eq!(identity.as_ref(), "SKU-1");
    }

    #[test]
    fn should_hash_the_stable_raw_stream_key_when_the_extracted_identity_is_blank()
    -> Result<(), NormalizationInputError> {
        let effective_url = Url::parse("https://example.com/products/moved")
            .unwrap_or_else(|error| panic!("static test URL must parse: {error}"));
        let raw_source_record_key = "https://example.com/products/original";
        let expected = normalize_source_listing_id_with_url_sha_fallback(
            "",
            &Url::parse(raw_source_record_key).unwrap(),
        )
        .unwrap();

        let identity = capture_source_listing_id("  ", raw_source_record_key)
            .unwrap_or_else(|error| panic!("URL hash identity must derive: {error}"));
        let mut extracted = raw();
        extracted.source_listing_id.clear();
        let input = crawler_raw_input(
            &extracted,
            &extracted.images,
            &effective_url,
            identity.as_ref(),
            None,
            None,
        )?;

        assert_eq!(identity, expected);
        assert_eq!(
            input.raw_values().value().get("sourceListingId"),
            Some(&serde_json::json!(expected.to_string()))
        );
        Ok(())
    }

    #[test]
    fn should_fail_closed_when_the_raw_stream_key_is_not_a_url() {
        assert!(matches!(
            capture_source_listing_id("", "not a url"),
            Err(NormalizationError::SourceListingIdInvalid(_))
        ));
    }

    #[test]
    fn should_preserve_image_groups_in_source_payload_and_use_validated_image_projection()
    -> Result<(), NormalizationInputError> {
        let url = Url::parse("https://example.com/products/1")
            .unwrap_or_else(|error| panic!("static test URL must parse: {error}"));
        let mut extracted = raw();
        extracted.images = vec![
            format!("/images/primary.jpg{IMAGE_CANDIDATE_SEPARATOR}/images/fallback-800x600.jpg"),
            format!(
                "/images/second.jpg{IMAGE_CANDIDATE_SEPARATOR}/images/second-fallback-640x480.jpg"
            ),
            format!("/images/primary.jpg{IMAGE_CANDIDATE_SEPARATOR}/images/fallback-800x600.jpg"),
        ];
        let validated_image_urls = vec![
            "https://example.com/images/fallback-800x600.jpg".to_owned(),
            "https://example.com/images/second-fallback-640x480.jpg".to_owned(),
        ];

        let input = crawler_raw_input(
            &extracted,
            &validated_image_urls,
            &url,
            "stable-id",
            Some(Currency::Eur),
            None,
        )?;

        assert_eq!(
            input.source_payload().value().get("images"),
            Some(&serde_json::json!([
                format!(
                    "/images/primary.jpg{IMAGE_CANDIDATE_SEPARATOR}/images/fallback-800x600.jpg"
                ),
                format!(
                    "/images/second.jpg{IMAGE_CANDIDATE_SEPARATOR}/images/second-fallback-640x480.jpg"
                ),
                format!(
                    "/images/primary.jpg{IMAGE_CANDIDATE_SEPARATOR}/images/fallback-800x600.jpg"
                ),
            ]))
        );
        assert_eq!(
            input.raw_values().value().get("images"),
            Some(&serde_json::json!({
                "action": "SET",
                "value": [
                    "https://example.com/images/fallback-800x600.jpg",
                    "https://example.com/images/second-fallback-640x480.jpg",
                ],
            }))
        );
        Ok(())
    }

    #[test]
    fn should_leave_a_blank_raw_attribute_out_of_the_raw_values_and_keep_it_in_the_source_payload()
    -> Result<(), NormalizationInputError> {
        let url = Url::parse("https://example.com/products/1")
            .unwrap_or_else(|error| panic!("static test URL must parse: {error}"));
        let mut extracted = raw();
        extracted
            .raw_attributes
            .insert("rawMaterial".to_owned(), vec!["Oak".to_owned()]);
        extracted
            .raw_attributes
            .insert("rawYear".to_owned(), vec![String::new()]);

        let input =
            crawler_raw_input(&extracted, &extracted.images, &url, "stable-id", None, None)?;

        assert_eq!(
            input.raw_values().value().get("attributes"),
            Some(&serde_json::json!({
                "rawMaterial": {"action": "SET", "value": ["Oak"]},
            }))
        );
        assert_eq!(
            input.source_payload().value().get("raw_attributes"),
            Some(&serde_json::json!({"rawMaterial": ["Oak"], "rawYear": [""]}))
        );
        Ok(())
    }

    #[test]
    fn should_hash_dynamic_raw_attribute_changes() -> Result<(), NormalizationInputError> {
        let url = Url::parse("https://example.com/products/1")
            .unwrap_or_else(|error| panic!("static test URL must parse: {error}"));
        let extracted = raw();
        let first = crawler_raw_input(
            &extracted,
            &extracted.images,
            &url,
            "stable-id",
            Some(Currency::Eur),
            None,
        )?
        .hash()?;
        let mut changed = raw();
        changed
            .raw_attributes
            .insert("rawMaterial".to_owned(), vec!["Oak".to_owned()]);
        let second = crawler_raw_input(
            &changed,
            &changed.images,
            &url,
            "stable-id",
            Some(Currency::Eur),
            None,
        )?
        .hash()?;
        assert_ne!(first, second);
        Ok(())
    }

    #[test]
    fn should_capture_a_carried_forward_title_and_keep_the_blank_extraction_as_evidence()
    -> Result<(), NormalizationInputError> {
        let url = Url::parse("https://example.com/products/1")
            .unwrap_or_else(|error| panic!("static test URL must parse: {error}"));
        let mut untitled = raw();
        untitled.title = String::new();
        untitled.state = "Sold".to_owned();

        let input = crawler_raw_input(
            &untitled,
            &untitled.images,
            &url,
            "stable-id",
            None,
            Some("Chair"),
        )?;

        assert_eq!(
            input.raw_values().value().get("title"),
            Some(&serde_json::json!({"action": "SET", "value": "Chair"}))
        );
        assert_eq!(
            input.raw_values().value().get("availability"),
            Some(&serde_json::json!({"action": "SET", "value": "Sold"}))
        );
        assert_eq!(
            input.source_payload().value().get("title"),
            Some(&serde_json::Value::String(String::new()))
        );
        Ok(())
    }
}
