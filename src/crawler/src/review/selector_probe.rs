//! What a few CSS selectors match on a page, without the page's outline: the review agents
//! check a selector, or find where a value sits, on pages whose outline they have not read or
//! read once (`POST /api/reviews/{id}/selector-probe`, `docs/crawler/operations.md`). A probe
//! extracts as runtime extraction does, every matched element, so what it shows is what a rule on
//! that selector would capture.

use crate::scraper::css_selector::rule::{
    CssSelector, ExtractionCardinality, ExtractionError, ExtractionKind, ExtractionRule,
    HtmlAttributeName,
};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};

/// The most selectors one probe applies to each page.
pub const MAX_PROBED_SELECTORS: usize = 8;

/// The most pages one probe fetches.
pub const MAX_PROBED_PAGES: usize = 4;

/// The most values a probe answers per selector and page: enough to tell one value from a list
/// and a product's own value from a related item's.
pub const MAX_PROBE_VALUES: usize = 3;

/// The characters a probed value keeps; a longer one ends in `…`.
pub const MAX_PROBE_VALUE_CHARS: usize = 300;

/// One selector to probe, read as text, or as the named attribute where the element has a
/// non-blank one and as its text otherwise (`attribute_or_text`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectorProbe {
    pub selector: CssSelector,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribute: Option<HtmlAttributeName>,
}

/// What one selector matched on one page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeResult {
    pub selector: CssSelector,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribute: Option<HtmlAttributeName>,
    /// Matched elements that hold a value.
    pub matches: usize,
    /// Matched elements without visible text, for a text probe.
    pub empty: usize,
    /// The first [`MAX_PROBE_VALUES`] values, each cut to [`MAX_PROBE_VALUE_CHARS`].
    pub values: Vec<String>,
    /// Why the selector could not be applied, such as an invalid selector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Applies each of `probes` to `html` on its own.
pub fn probe(html: &Html, probes: &[SelectorProbe]) -> Vec<ProbeResult> {
    probes.iter().map(|probe| probe_one(html, probe)).collect()
}

fn probe_one(html: &Html, probe: &SelectorProbe) -> ProbeResult {
    let rule = ExtractionRule {
        selector: probe.selector.clone(),
        additional_selectors: Vec::new(),
        extract: match &probe.attribute {
            Some(name) => ExtractionKind::AttributeOrText { name: name.clone() },
            None => ExtractionKind::Text,
        },
        cardinality: ExtractionCardinality::All,
    };
    let result = |matches, empty, values, error| ProbeResult {
        selector: probe.selector.clone(),
        attribute: probe.attribute.clone(),
        matches,
        empty,
        values,
        error,
    };
    let parsed = match rule
        .validate()
        .and_then(|()| Selector::parse(&probe.selector).map_err(|_| invalid(&probe.selector)))
    {
        Ok(parsed) => parsed,
        Err(error) => return result(0, 0, Vec::new(), Some(error.to_string())),
    };
    let matches = rule.match_count(html, &parsed);
    let empty = rule.empty_element_count(html, &parsed);
    match rule.apply(html) {
        Ok(values) => result(matches, empty, shortened(values), None),
        Err(ExtractionError::NoElementMatched { .. } | ExtractionError::EmptyValue { .. }) => {
            result(matches, empty, Vec::new(), None)
        }
        Err(error) => result(matches, empty, Vec::new(), Some(error.to_string())),
    }
}

fn invalid(selector: &CssSelector) -> ExtractionError {
    ExtractionError::InvalidSelector {
        selector: selector.to_string(),
        reason: "the selector does not parse".to_owned(),
    }
}

fn shortened(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .take(MAX_PROBE_VALUES)
        .map(|value| {
            if value.chars().count() <= MAX_PROBE_VALUE_CHARS {
                value
            } else {
                let mut cut: String = value.chars().take(MAX_PROBE_VALUE_CHARS).collect();
                cut.push('…');
                cut
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<html><body>
        <h1 class="title">Georgian Chair</h1>
        <p class="price">£450</p>
        <p class="price">£45 shipping</p>
        <p class="price">£60 insurance</p>
        <p class="price">£5 fee</p>
        <span class="empty"> </span>
        <form id="product_form_8721"><input name="product-id" value="8721">
        <button>Add to basket</button></form>
    </body></html>"#;

    fn probe_page(probes: &[SelectorProbe]) -> Vec<ProbeResult> {
        probe(&Html::parse_document(PAGE), probes)
    }

    fn text(selector: &str) -> SelectorProbe {
        SelectorProbe {
            selector: CssSelector::from(selector),
            attribute: None,
        }
    }

    #[test]
    fn should_answer_the_text_of_each_selector_on_its_own() {
        let results = probe_page(&[text("h1.title"), text("button")]);

        assert_eq!(results[0].values, ["Georgian Chair"]);
        assert_eq!(results[0].matches, 1);
        assert_eq!(results[1].values, ["Add to basket"]);
    }

    #[test]
    fn should_answer_an_attribute_and_fall_back_to_the_text() {
        let results = probe_page(&[
            SelectorProbe {
                selector: CssSelector::from("input[name=product-id]"),
                attribute: Some(HtmlAttributeName::from("value")),
            },
            SelectorProbe {
                selector: CssSelector::from("form[id^=product_form_]"),
                attribute: Some(HtmlAttributeName::from("id")),
            },
            SelectorProbe {
                selector: CssSelector::from("button"),
                attribute: Some(HtmlAttributeName::from("value")),
            },
        ]);

        assert_eq!(results[0].values, ["8721"]);
        assert_eq!(results[1].values, ["product_form_8721"]);
        assert_eq!(results[2].values, ["Add to basket"]);
    }

    #[test]
    fn should_count_every_match_but_answer_only_the_first_values() {
        let [prices] = probe_page(&[text("p.price")]).try_into().unwrap();

        assert_eq!(prices.matches, 4);
        assert_eq!(prices.values, ["£450", "£45 shipping", "£60 insurance"]);
    }

    #[test]
    fn should_tell_an_empty_element_from_a_selector_that_matches_nothing() {
        let results = probe_page(&[text("span.empty"), text("span.missing")]);

        assert_eq!((results[0].matches, results[0].empty), (0, 1));
        assert_eq!((results[1].matches, results[1].empty), (0, 0));
        assert!(results.iter().all(|result| result.values.is_empty()));
        assert!(results.iter().all(|result| result.error.is_none()));
    }

    #[test]
    fn should_name_an_invalid_selector_and_probe_the_others() {
        let results = probe_page(&[text("h1["), text("h1.title")]);

        assert!(
            results[0]
                .error
                .as_deref()
                .is_some_and(|error| error.contains("h1["))
        );
        assert_eq!(results[1].values, ["Georgian Chair"]);
    }

    #[test]
    fn should_cut_a_long_value() {
        let html = Html::parse_document(&format!("<p>{}</p>", "a".repeat(400)));

        let [result] = probe(&html, &[text("p")]).try_into().unwrap();

        assert_eq!(result.values[0].chars().count(), MAX_PROBE_VALUE_CHARS + 1);
        assert!(result.values[0].ends_with('…'));
    }
}
