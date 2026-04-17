use crawler::scraper::css_selector::rule::{
    CssSelector, ExtractionCardinality, ExtractionKind, ExtractionRule, HtmlAttributeName,
};

pub fn text_rule(selector: &str) -> ExtractionRule {
    ExtractionRule {
        selector: CssSelector::from(selector),
        additional_selectors: vec![],
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::First,
    }
}

pub fn text_rule_with_fallbacks(selector: &str, additional: &[&str]) -> ExtractionRule {
    ExtractionRule {
        selector: CssSelector::from(selector),
        additional_selectors: additional.iter().map(|s| CssSelector::from(*s)).collect(),
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::First,
    }
}

pub fn attr_rule_first(selector: &str, attr: &str, additional: &[&str]) -> ExtractionRule {
    ExtractionRule {
        selector: CssSelector::from(selector),
        additional_selectors: additional.iter().map(|s| CssSelector::from(*s)).collect(),
        extract: ExtractionKind::Attribute {
            name: HtmlAttributeName::from(attr),
        },
        cardinality: ExtractionCardinality::First,
    }
}

pub fn attr_rule_all(selector: &str, attr: &str) -> ExtractionRule {
    ExtractionRule {
        selector: CssSelector::from(selector),
        additional_selectors: vec![],
        extract: ExtractionKind::Attribute {
            name: HtmlAttributeName::from(attr),
        },
        cardinality: ExtractionCardinality::All,
    }
}
