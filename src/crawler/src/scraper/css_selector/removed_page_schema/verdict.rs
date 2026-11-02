use scraper::Html;

use super::{RemovedPageMatch, RemovedPageSchema};
use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use crate::scraper::raw_input::capture_source_listing_id;

/// What a page's removed-page templates say about it once the product schemas' extractions of
/// the same page are weighed against them. The scraper's removal guard and the review's schema
/// matrix both reach it through [`removed_page_verdict`], so a reviewer sees what the scraper
/// will do with a page before approving a template.
#[derive(Debug)]
pub enum RemovedPageVerdict {
    /// No template matches the page.
    NotRemoved,
    /// A template matches and no product schema still extracts this listing's product from the
    /// page.
    Removed { matches: Vec<RemovedPageMatch> },
    /// A template matches, but a product schema still extracts this listing's product: the
    /// template is over-broad for this page.
    ProductStillExtracts { matches: Vec<RemovedPageMatch> },
}

/// The templates of `removed_page_schemas` that match the parsed page, each with its position
/// in the list and its notices.
pub fn match_removed_page(
    removed_page_schemas: impl IntoIterator<Item = RemovedPageSchema>,
    parsed: &Html,
) -> Vec<RemovedPageMatch> {
    removed_page_schemas
        .into_iter()
        .enumerate()
        .filter_map(|(template_index, schema)| schema.match_page(template_index, parsed))
        .collect()
}

/// Weighs the templates that match a page against what the product schemas extract from it.
///
/// Product evidence is an extraction with a title other than the matched notice and, when the
/// crawler already knows the listing's identity, the same source listing id. A related-item card
/// on a soft-404 page therefore does not count as evidence. Plausibility is deliberately not
/// required: an extraction that fails a plausibility check must reach schema work, not
/// withdrawal. `source_record_key` stands in for an identity the page does not yield.
pub fn removed_page_verdict<'a>(
    matches: Vec<RemovedPageMatch>,
    extractions: impl IntoIterator<Item = &'a RawExtractedProduct>,
    source_record_key: &str,
    last_source_listing_id: Option<&str>,
) -> RemovedPageVerdict {
    if matches.is_empty() {
        return RemovedPageVerdict::NotRemoved;
    }

    let product_still_extracts = extractions.into_iter().any(|raw| {
        let title = raw.title.trim();
        if title.is_empty() || matches.iter().any(|matched| matched.is_notice(title)) {
            return false;
        }
        last_source_listing_id.is_none_or(|known| {
            captured_identity(&raw.source_listing_id, source_record_key)
                .is_some_and(|extracted| extracted == known)
        })
    });
    if product_still_extracts {
        RemovedPageVerdict::ProductStillExtracts { matches }
    } else {
        RemovedPageVerdict::Removed { matches }
    }
}

/// The identity a raw extraction would be captured under, so it compares with the identity the
/// listing was last captured under.
fn captured_identity(raw_source_listing_id: &str, source_record_key: &str) -> Option<String> {
    capture_source_listing_id(raw_source_listing_id, source_record_key)
        .ok()
        .map(|id| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::css_selector::rule::CssSelector;
    use std::collections::BTreeMap;

    const RECORD_KEY: &str = "https://shop.example/item/42";
    const NOTICE_PAGE: &str =
        r#"<main><h1>This item is no longer available</h1><h2>Biedermeier Chair</h2></main>"#;

    fn notice_template(text: &str) -> RemovedPageSchema {
        RemovedPageSchema {
            selector: CssSelector::from("main h1"),
            text: Some(text.to_owned()),
            regex: None,
        }
    }

    fn extraction(title: &str, source_listing_id: &str) -> RawExtractedProduct {
        RawExtractedProduct {
            source_listing_id: source_listing_id.to_owned(),
            title: title.to_owned(),
            description: vec![],
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: "Sold".to_owned(),
            images: vec![],
            raw_attributes: BTreeMap::new(),
        }
    }

    fn verdict(
        templates: Vec<RemovedPageSchema>,
        extractions: &[RawExtractedProduct],
        last_source_listing_id: Option<&str>,
    ) -> RemovedPageVerdict {
        let matches = match_removed_page(templates, &Html::parse_document(NOTICE_PAGE));
        removed_page_verdict(matches, extractions, RECORD_KEY, last_source_listing_id)
    }

    #[test]
    fn should_keep_each_identical_template_at_its_own_index() {
        let template = notice_template("no longer available");

        let matches = match_removed_page(
            vec![template.clone(), notice_template("sold out"), template],
            &Html::parse_document(NOTICE_PAGE),
        );

        assert_eq!(
            matches
                .iter()
                .map(|matched| matched.template_index)
                .collect::<Vec<_>>(),
            vec![0, 2]
        );
    }

    #[test]
    fn should_find_a_page_removed_when_the_only_extracted_title_is_the_notice() {
        let extractions = [extraction("This item is no longer available", "SKU-42")];

        let verdict = verdict(
            vec![notice_template("no longer available")],
            &extractions,
            None,
        );

        assert!(matches!(verdict, RemovedPageVerdict::Removed { .. }));
    }

    #[test]
    fn should_keep_a_product_page_whose_listing_still_extracts() {
        let extractions = [extraction("Biedermeier Chair", "SKU-42")];

        let verdict = verdict(
            vec![notice_template("no longer available")],
            &extractions,
            Some("SKU-42"),
        );

        assert!(matches!(
            verdict,
            RemovedPageVerdict::ProductStillExtracts { .. }
        ));
    }

    #[test]
    fn should_not_count_another_listing_as_product_evidence() {
        let extractions = [extraction("Biedermeier Chair", "SKU-7")];

        let verdict = verdict(
            vec![notice_template("no longer available")],
            &extractions,
            Some("SKU-42"),
        );

        assert!(matches!(verdict, RemovedPageVerdict::Removed { .. }));
    }

    #[test]
    fn should_not_remove_a_page_no_template_matches() {
        let extractions = [extraction("Biedermeier Chair", "SKU-42")];

        let verdict = verdict(vec![notice_template("sold out")], &extractions, None);

        assert!(matches!(verdict, RemovedPageVerdict::NotRemoved));
    }
}
