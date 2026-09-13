//! Fixture-backed Auction evidence extractors for known source URL namespaces.
//!
//! This module intentionally has no generic URL matching. A provider rule must prove
//! both its source-key path and any page selectors with a checked-in fixture.

use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CrawlerAuctionEvidence {
    /// A source key is reliable identity. Its absence still permits a reliable
    /// participation assertion when a source-specific extractor proves one.
    pub(crate) source_auction_id: Option<String>,
    pub(crate) catalogue_url: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) lot_number: Option<String>,
    pub(crate) lot_bidding_opens: Option<String>,
    pub(crate) lot_scheduled_closes: Option<String>,
}

/// Extracts Lot-tissimo catalogue evidence from its tested lot URL namespace.
///
/// The source key is the nonempty suffix of `catalogue-id-…`. The rule requires
/// the complete known path and never falls back to a name, generic URL hash, or
/// a loosely matched path segment.
pub(crate) fn extract_lot_tissimo_auction(
    candidate_url: &Url,
    raw: &RawExtractedProduct,
    html: &str,
) -> Option<CrawlerAuctionEvidence> {
    let host = candidate_url.host_str()?;
    if !matches!(host, "lot-tissimo.com" | "www.lot-tissimo.com")
        || candidate_url.scheme() != "https"
        || candidate_url.query().is_some()
        || candidate_url.fragment().is_some()
    {
        return None;
    }

    let segments = candidate_url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let [locale, catalogue_collection, auctioneer, catalogue, lot] = segments.as_slice() else {
        return None;
    };
    if !is_lot_tissimo_locale(locale)
        || *catalogue_collection != "auction-catalogues"
        || auctioneer.is_empty()
    {
        return None;
    }
    let source_auction_id = catalogue.strip_prefix("catalogue-id-")?;
    if source_auction_id.is_empty()
        || lot
            .strip_prefix("lot-")
            .is_none_or(|source_lot_id| source_lot_id.is_empty())
    {
        return None;
    }

    let mut catalogue_url = candidate_url.clone();
    catalogue_url.set_path(&format!(
        "/{locale}/{catalogue_collection}/{auctioneer}/{catalogue}"
    ));
    catalogue_url.set_query(None);
    catalogue_url.set_fragment(None);
    let catalogue_url = catalogue_url.to_string();

    Some(CrawlerAuctionEvidence {
        source_auction_id: Some(source_auction_id.to_owned()),
        name: raw_attribute(raw, "rawAuctionName"),
        catalogue_url: Some(catalogue_url),
        lot_number: raw_attribute(raw, "rawAuctionLotNumber"),
        // These source data-layer fields are date-only. They are lot facts,
        // not Auction schedule facts, and a `Live` type label is not a close.
        lot_bidding_opens: data_layer_string(html, "lotStartDate"),
        lot_scheduled_closes: data_layer_string(html, "lotEndDate"),
    })
}

/// Reads one exact quoted data-layer value. The caller has already qualified the
/// source and URL namespace; this parser never scans generic crawler pages.
fn data_layer_string(html: &str, field: &str) -> Option<String> {
    let key = format!("\"{field}\"");
    let after_key = html.split_once(key.as_str())?.1;
    let after_colon = after_key.split_once(':')?.1;
    let value = after_colon.trim_start().strip_prefix('\"')?;
    let value = value.split_once('\"')?.0.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn is_lot_tissimo_locale(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 5
        && bytes[2] == b'-'
        && bytes[0].is_ascii_lowercase()
        && bytes[1].is_ascii_lowercase()
        && bytes[3].is_ascii_lowercase()
        && bytes[4].is_ascii_lowercase()
}

fn raw_attribute(raw: &RawExtractedProduct, key: &str) -> Option<String> {
    raw.raw_attributes
        .get(key)
        .and_then(|values| values.first())
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
    use scraper::Html;
    use serde_json::Value;

    const LOT_TISSIMO_HTML: &str =
        include_str!("../../tests/fixtures/html/lot-tissimo_listed.html");
    const FIXTURES: &str = include_str!("../../tests/fixtures/fixtures.json");
    const LOT_URL: &str = "https://www.lot-tissimo.com/de-de/auction-catalogues/kunstauktionshaus-leipzig/catalogue-id-leipzig10033/lot-a2850590-e73c-4cce-9386-b3fd00b49bfd";

    fn raw() -> RawExtractedProduct {
        let fixtures: Value = serde_json::from_str(FIXTURES)
            .unwrap_or_else(|error| panic!("crawler fixture JSON: {error}"));
        let fixture = fixtures
            .as_array()
            .and_then(|fixtures| {
                fixtures.iter().find(|fixture| {
                    fixture.get("html").and_then(Value::as_str)
                        == Some("tests/fixtures/html/lot-tissimo_listed.html")
                })
            })
            .unwrap_or_else(|| panic!("Lot-tissimo fixture must exist"));
        let schema: ProductCssSelectorSchema = serde_json::from_value(
            fixture
                .get("schema")
                .cloned()
                .unwrap_or_else(|| panic!("Lot-tissimo fixture schema must exist")),
        )
        .unwrap_or_else(|error| panic!("Lot-tissimo fixture schema: {error}"));
        schema
            .apply(&Html::parse_document(LOT_TISSIMO_HTML))
            .unwrap_or_else(|error| panic!("Lot-tissimo fixture schema must apply: {error}"))
    }

    #[test]
    fn should_extract_catalogue_identity_without_using_live_banner_or_opening_price() {
        let url = Url::parse(LOT_URL).unwrap_or_else(|error| panic!("fixture URL: {error}"));

        let evidence = extract_lot_tissimo_auction(&url, &raw(), LOT_TISSIMO_HTML)
            .unwrap_or_else(|| panic!("fixture must match documented Lot-tissimo rule"));

        assert_eq!(Some("leipzig10033".to_owned()), evidence.source_auction_id);
        assert_eq!(
            Some(
                "https://www.lot-tissimo.com/de-de/auction-catalogues/kunstauktionshaus-leipzig/catalogue-id-leipzig10033".to_owned(),
            ),
            evidence.catalogue_url
        );
        assert_eq!(Some("Auktion 9".to_owned()), evidence.name);
        assert_ne!(Some("Live auf Los 54".to_owned()), evidence.name);
        assert_eq!(None, raw().price, "openingPrice is not a listing price");
        assert_eq!(None, raw().price_estimate_min);
        assert_eq!(None, raw().price_estimate_max);
        assert_eq!(Some("54".to_owned()), evidence.lot_number);
        assert_eq!(Some("2026-04-18".to_owned()), evidence.lot_bidding_opens);
        assert_eq!(None, evidence.lot_scheduled_closes);
    }

    #[test]
    fn should_reject_unproven_hosts_wrappers_and_path_shapes() {
        let raw = raw();
        for url in [
            "https://example.test/de-de/auction-catalogues/kunstauktionshaus-leipzig/catalogue-id-leipzig10033/lot-a2850590-e73c-4cce-9386-b3fd00b49bfd",
            "https://www.lot-tissimo.com/de-de/auction-catalogues/kunstauktionshaus-leipzig/catalogue-id-/lot-a2850590-e73c-4cce-9386-b3fd00b49bfd",
            "https://www.lot-tissimo.com/de-de/auction-catalogues/kunstauktionshaus-leipzig/catalogue-id-leipzig10033",
            "https://www.lot-tissimo.com/de-de/auction-catalogues/kunstauktionshaus-leipzig/catalogue-id-leipzig10033/lot-a2850590-e73c-4cce-9386-b3fd00b49bfd?utm_source=fixture",
        ] {
            let url = Url::parse(url).unwrap_or_else(|error| panic!("fixture URL: {error}"));
            assert!(
                extract_lot_tissimo_auction(&url, &raw, LOT_TISSIMO_HTML).is_none(),
                "{url}"
            );
        }
    }

    #[test]
    fn should_map_lot_end_date_to_lot_close_without_treating_live_as_a_close() {
        let html = LOT_TISSIMO_HTML.replacen(
            "\"lotEndDate\" : \"\"",
            "\"lotEndDate\" : \"2026-04-18\"",
            1,
        );
        let url = Url::parse(LOT_URL).unwrap_or_else(|error| panic!("fixture URL: {error}"));

        let evidence = extract_lot_tissimo_auction(&url, &raw(), &html)
            .unwrap_or_else(|| panic!("fixture URL must remain qualified"));

        assert_eq!(Some("2026-04-18".to_owned()), evidence.lot_bidding_opens);
        assert_eq!(Some("2026-04-18".to_owned()), evidence.lot_scheduled_closes);
    }
}
