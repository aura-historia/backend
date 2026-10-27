use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectorFieldEvaluation {
    pub field: String,
    pub selector: String,
    /// Elements the primary selector matched, `None` when it does not parse. A text rule
    /// counts only elements with visible text, because a blank one yields no usable
    /// value.
    pub selector_match_count: Option<usize>,
    /// The same count for each additional selector, `0` for one that does not parse.
    pub additional_selector_match_counts: Vec<usize>,
    pub error: Option<String>,
    /// Whether a miss of this rule fails the page (`title`, `state`). Set by the
    /// evaluator from the schema field it iterates, never derived from the name.
    #[serde(default)]
    pub required: bool,
    /// Matched elements the `images` rule skipped on this page because they hold no
    /// value (no image URL candidate, or no attribute of an attribute rule), such as
    /// `href="#"` anchors in a gallery. Next to extracted images a count above zero shows
    /// a loose selector; on a page whose gallery holds only placeholders it counts every
    /// matched element and the page has no images. `None` for every other rule, which
    /// never skips an element, and when the `images` rule failed the page.
    #[serde(default)]
    pub skipped_element_count: Option<usize>,
    /// Elements a text rule's selectors matched that hold no visible text, summed over the
    /// primary and additional selectors; `0` for every other rule, whose match counts
    /// include every element. When no element with a value matched, a count above zero
    /// tells a page that leaves the field empty from a selector that matched nothing.
    #[serde(default)]
    pub empty_element_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A matrix cached before the `images` rule skipped elements, or before empty elements
    /// were counted, has neither count. It still loads: an `images` rule that applied then
    /// had no element to skip, one that failed the page reports no count either, and an
    /// unknown empty-element count reads as none.
    #[test]
    fn should_load_a_cached_field_evaluation_without_skipped_or_empty_element_counts() {
        let field = serde_json::json!({
            "field": "images",
            "selector": "#gallery a",
            "selector_match_count": 3,
            "additional_selector_match_counts": [],
            "error": null,
            "required": false
        });

        let field = serde_json::from_value::<SelectorFieldEvaluation>(field)
            .expect("a cached field evaluation without the count must load");

        assert_eq!(field.skipped_element_count, None);
        assert_eq!(field.empty_element_count, 0);
    }
}
