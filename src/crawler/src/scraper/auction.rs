//! Fixture-backed Auction evidence extractors for known source URL namespaces.
//!
//! This module intentionally has no generic URL matching. A provider rule must prove
//! both its source-key path and any page selectors with a checked-in fixture.

use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use serde_json::Value;
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
    let LotTissimoLotUrl {
        locale,
        catalogue_collection,
        auctioneer,
        catalogue,
        source_auction_id,
        source_lot_id,
    } = lot_tissimo_lot_url(candidate_url)?;

    let mut catalogue_url = candidate_url.clone();
    catalogue_url.set_path(&format!(
        "/{locale}/{catalogue_collection}/{auctioneer}/{catalogue}"
    ));
    catalogue_url.set_query(None);
    catalogue_url.set_fragment(None);
    let catalogue_url = catalogue_url.to_string();

    let dates = data_layer_lot_dates(html, source_lot_id.as_str(), source_auction_id.as_str());

    Some(CrawlerAuctionEvidence {
        source_auction_id: Some(source_auction_id),
        name: raw_attribute(raw, "rawAuctionName"),
        catalogue_url: Some(catalogue_url),
        lot_number: raw_attribute(raw, "rawAuctionLotNumber"),
        // These source data-layer fields are date-only. They are lot facts,
        // not Auction schedule facts, and a `Live` type label is not a close.
        lot_bidding_opens: dates.as_ref().and_then(|dates| dates.bidding_opens.clone()),
        lot_scheduled_closes: dates.and_then(|dates| dates.scheduled_closes),
    })
}

/// Returns whether this URL can use the fixture-backed Lot-tissimo evidence rule.
///
/// Its auction facts may occur outside `<main>`, so the scraper must fingerprint
/// the full source document before allowing the main-only fast path.
pub(crate) fn requires_lot_tissimo_full_document_fingerprint(candidate_url: &Url) -> bool {
    lot_tissimo_lot_url(candidate_url).is_some()
}

struct LotTissimoLotUrl {
    locale: String,
    catalogue_collection: String,
    auctioneer: String,
    catalogue: String,
    source_auction_id: String,
    source_lot_id: String,
}

struct LotTissimoLotDates {
    bidding_opens: Option<String>,
    scheduled_closes: Option<String>,
}

fn lot_tissimo_lot_url(candidate_url: &Url) -> Option<LotTissimoLotUrl> {
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
    let source_lot_id = lot.strip_prefix("lot-")?;
    if source_auction_id.is_empty() || source_lot_id.is_empty() {
        return None;
    }

    Some(LotTissimoLotUrl {
        locale: (*locale).to_owned(),
        catalogue_collection: (*catalogue_collection).to_owned(),
        auctioneer: (*auctioneer).to_owned(),
        catalogue: (*catalogue).to_owned(),
        source_auction_id: source_auction_id.to_owned(),
        source_lot_id: source_lot_id.to_owned(),
    })
}

/// Selects only the Lot-tissimo data-layer record for this URL's lot and catalogue.
///
/// The provider pushes JavaScript-wrapped object literals. This scanner extracts
/// balanced object payloads without executing JavaScript, then decodes their
/// fixture-backed JSON object shape. Conflicting matching records are omitted
/// rather than letting document order choose canonical lot timing.
fn data_layer_lot_dates(
    html: &str,
    source_lot_id: &str,
    source_auction_id: &str,
) -> Option<LotTissimoLotDates> {
    let matching_records = data_layer_objects(html)
        .into_iter()
        .filter(|record| {
            record.get("lotId").and_then(Value::as_str) == Some(source_lot_id)
                && record.get("catalogueId").and_then(Value::as_str) == Some(source_auction_id)
        })
        .collect::<Vec<_>>();
    (!matching_records.is_empty()).then(|| LotTissimoLotDates {
        bidding_opens: agreed_data_layer_date(&matching_records, "lotStartDate"),
        scheduled_closes: agreed_data_layer_date(&matching_records, "lotEndDate"),
    })
}

fn agreed_data_layer_date(records: &[Value], field: &str) -> Option<String> {
    let values = records
        .iter()
        .map(|record| {
            record
                .get(field)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .collect::<std::collections::BTreeSet<_>>();
    (values.len() == 1)
        .then(|| values.into_iter().next().flatten())
        .flatten()
}

fn data_layer_objects(html: &str) -> Vec<Value> {
    const PREFIX: &str = "window.dataLayer.push(";
    let mut remaining = html;
    let mut records = Vec::new();
    while let Some(after_prefix) = remaining.split_once(PREFIX).map(|(_, value)| value) {
        let Some(object_start) = after_prefix.find('{') else {
            remaining = after_prefix;
            continue;
        };
        let after_start = &after_prefix[object_start..];
        let Some(object_len) = balanced_object_len(after_start) else {
            remaining = after_start;
            continue;
        };
        if let Some(record) = parse_data_layer_object(&after_start[..object_len]) {
            records.push(record);
        }
        remaining = &after_start[object_len..];
    }
    records
}

fn parse_data_layer_object(value: &str) -> Option<Value> {
    if let Ok(record) = serde_json::from_str(value) {
        return Some(record);
    }

    // The checked-in provider object is JSON-shaped JavaScript with one trailing
    // property comma. Permit exactly that source syntax after object bounds are
    // proven; do not evaluate or broadly rewrite fetched script.
    let before_closing_brace = value.strip_suffix('}')?.trim_end();
    let before_comma = before_closing_brace.strip_suffix(',')?.trim_end();
    serde_json::from_str(&format!("{before_comma}}}")).ok()
}

fn balanced_object_len(value: &str) -> Option<usize> {
    let mut depth = 0_u32;
    let mut quote = None;
    let mut escaped = false;
    for (index, byte) in value.bytes().enumerate() {
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == delimiter {
                quote = None;
            }
            continue;
        }
        match byte {
            b'\'' | b'\"' => quote = Some(byte),
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index + 1);
                }
            }
            _ => {}
        }
    }
    None
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
    fn should_require_full_document_fingerprint_for_qualified_lot_tissimo_lot_urls() {
        let qualified = Url::parse(LOT_URL).unwrap_or_else(|error| panic!("fixture URL: {error}"));
        let non_auction = Url::parse("https://www.lot-tissimo.com/de-de/catalogues")
            .unwrap_or_else(|error| panic!("fixture URL: {error}"));

        assert!(requires_lot_tissimo_full_document_fingerprint(&qualified));
        assert!(!requires_lot_tissimo_full_document_fingerprint(
            &non_auction
        ));
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
    fn should_use_only_the_data_layer_record_for_the_current_lot() {
        let html = LOT_TISSIMO_HTML
            .replacen(
                "<script> window.dataLayer = window.dataLayer || [];",
                r#"<script>window.dataLayer.push({"lotId":"other-lot","catalogueId":"leipzig10033","lotEndDate":"2026-12-31"});</script>
<script> window.dataLayer = window.dataLayer || [];"#,
                1,
            )
            .replacen(
                "\"lotEndDate\" : \"\"",
                "\"lotEndDate\" : \"2026-04-18\"",
                1,
            );
        let url = Url::parse(LOT_URL).unwrap_or_else(|error| panic!("fixture URL: {error}"));

        let evidence = extract_lot_tissimo_auction(&url, &raw(), &html)
            .unwrap_or_else(|| panic!("fixture URL must remain qualified"));

        assert_eq!(Some("2026-04-18".to_owned()), evidence.lot_scheduled_closes);
    }

    #[test]
    fn should_omit_conflicting_dates_from_multiple_matching_data_layer_records() {
        let html = LOT_TISSIMO_HTML
            .replacen(
                "\"lotEndDate\" : \"\"",
                "\"lotEndDate\" : \"2026-04-18\"",
                1,
            )
            .replacen(
                "</script>",
                r#"window.dataLayer.push({"lotId":"a2850590-e73c-4cce-9386-b3fd00b49bfd","catalogueId":"leipzig10033","lotEndDate":"2026-04-19"});</script>"#,
                1,
            );
        let url = Url::parse(LOT_URL).unwrap_or_else(|error| panic!("fixture URL: {error}"));

        let evidence = extract_lot_tissimo_auction(&url, &raw(), &html)
            .unwrap_or_else(|| panic!("fixture URL must remain qualified"));

        assert_eq!(None, evidence.lot_scheduled_closes);
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
