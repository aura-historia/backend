//! Bounded item assertions. Full clauses must match; arbitrary substrings never authorize a write.
mod lexicon;

use lexicon::LANGUAGES;
use localization::Language;
use regex::Regex;
use shipping_core::{
    pricing::ShippingPrice,
    time::{TimeBasis, TimeRange, TimeUnit},
};
use std::sync::LazyLock;

pub(crate) const MAX_SHIPPING_TEXT_BYTES: usize = 4096;
const MAX_SHIPPING_TEXT_FRAGMENTS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemShippingAssertions {
    /// Collection-only wording prohibits shipping without changing the collection component.
    pub prohibited: bool,
    pub worldwide: bool,
    pub offered: bool,
    pub price: Option<ShippingPrice>,
    pub handling: Option<TimeRange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShippingQuickCheck {
    Resolved(ItemShippingAssertions),
    NoAssertion,
    Unsupported,
}

static DISPATCH: LazyLock<Result<Vec<Regex>, regex::Error>> = LazyLock::new(|| {
    LANGUAGES
        .iter()
        .map(|rules| {
            Regex::new(&format!(
                r"^(?:{})(?P<minimum>[0-9]{{1,3}})(?:\s*(?:{})\s*(?P<maximum>[0-9]{{1,3}}))?(?:{})$",
                rules.dispatch_prefix, rules.range_separator, rules.dispatch_suffix,
            ))
        })
        .collect()
});

/// Matches whole, explicit item clauses in every supported catalog language. Unknown
/// delivery or conditional clauses veto the observation, including across languages.
#[cfg(test)]
fn quick_check_shipping(fragments: &[&str]) -> ShippingQuickCheck {
    check_shipping(fragments, 0)
}

/// Dedicated shipping fields carry delivery context even when a clause omits that word
/// (for example, a note saying "Only within Europe"). Description prose remains context.
pub fn quick_check_shipping_with_context(
    shipping_fields: &[&str],
    description: &[&str],
) -> ShippingQuickCheck {
    if shipping_fields.len() > MAX_SHIPPING_TEXT_FRAGMENTS
        || description.len() > MAX_SHIPPING_TEXT_FRAGMENTS - shipping_fields.len()
    {
        return ShippingQuickCheck::Unsupported;
    }
    let fragments = shipping_fields
        .iter()
        .chain(description)
        .copied()
        .collect::<Vec<_>>();
    check_shipping(&fragments, shipping_fields.len())
}

fn check_shipping(fragments: &[&str], shipping_field_count: usize) -> ShippingQuickCheck {
    if fragments.len() > MAX_SHIPPING_TEXT_FRAGMENTS
        || fragments.iter().map(|s| s.len()).sum::<usize>() > MAX_SHIPPING_TEXT_BYTES
        || fragments.iter().any(|s| s.contains(['\0', '?', '？', '؟']))
    {
        return ShippingQuickCheck::Unsupported;
    }
    let Ok(dispatch) = DISPATCH.as_ref() else {
        return ShippingQuickCheck::Unsupported;
    };
    let mut assertions = ItemShippingAssertions::default();
    let mut collection_label = false;
    let mut found = false;
    for (index, fragment) in fragments.iter().enumerate() {
        let normalized = normalize_text(fragment);
        for clause in normalized.split(['.', '!', ';', '\n', '\r', '。', '！', '；', '؛']) {
            let clause = clause.split_whitespace().collect::<Vec<_>>().join(" ");
            if clause.is_empty() {
                continue;
            }
            match statement(&clause, dispatch) {
                Some(Statement::Prohibited) => assertions.prohibited = true,
                Some(Statement::CollectionOnly) => collection_label = true,
                Some(Statement::Worldwide) => {
                    assertions.worldwide = true;
                    assertions.offered = true;
                }
                Some(Statement::Price(price)) => {
                    if assertions.price.is_some_and(|previous| previous != price) {
                        return ShippingQuickCheck::Unsupported;
                    }
                    assertions.worldwide = true;
                    assertions.price = Some(price);
                }
                Some(Statement::Handling(range)) => {
                    if assertions
                        .handling
                        .is_some_and(|previous| previous != range)
                    {
                        return ShippingQuickCheck::Unsupported;
                    }
                    assertions.handling = Some(range);
                }
                Some(Statement::Invalid) => return ShippingQuickCheck::Unsupported,
                None => {
                    if index < shipping_field_count
                        || delivery_related(&clause)
                        || conditional(&clause)
                    {
                        return ShippingQuickCheck::Unsupported;
                    }
                    continue;
                }
            }
            found = true;
        }
    }
    if contradictory(&assertions, collection_label) {
        ShippingQuickCheck::Unsupported
    } else if found {
        ShippingQuickCheck::Resolved(assertions)
    } else {
        ShippingQuickCheck::NoAssertion
    }
}

fn contradictory(assertions: &ItemShippingAssertions, collection_label: bool) -> bool {
    (collection_label && !assertions.prohibited)
        || (assertions.prohibited
            && (assertions.worldwide
                || assertions.price.is_some()
                || assertions.handling.is_some()))
}

/// Case and layout only. Digits have the same decimal value in these scripts. Never
/// strip accents, translate, change geographic scope, or interpret other numeral systems.
fn normalize_text(text: &str) -> String {
    text.chars()
        // Map Turkish dotted capital I before generic lowercasing, preserving unrelated
        // combining accents (including Polish dotted letters) in the original text.
        .map(|c| if c == 'İ' { 'i' } else { c })
        .flat_map(char::to_lowercase)
        .filter_map(|c| match c {
            '’' | 'ʼ' => Some('\''),
            '‐' | '‑' | '‒' | '–' | '—' | '−' | '〜' | '～' => Some('-'),
            '٠'..='٩' => char::from_u32(u32::from(c) - u32::from('٠') + u32::from('0')),
            '۰'..='۹' => char::from_u32(u32::from(c) - u32::from('۰') + u32::from('0')),
            '０'..='９' => char::from_u32(u32::from(c) - u32::from('０') + u32::from('0')),
            '\u{200e}' | '\u{200f}' => None,
            _ => Some(c),
        })
        .collect()
}

enum Statement {
    Prohibited,
    CollectionOnly,
    Worldwide,
    Price(ShippingPrice),
    Handling(TimeRange),
    Invalid,
}

fn statement(clause: &str, dispatch: &[Regex]) -> Option<Statement> {
    for rules in LANGUAGES {
        if phrase_matches(rules.language, rules.prohibited, clause) {
            return Some(Statement::Prohibited);
        }
        if phrase_matches(rules.language, rules.collection_only, clause) {
            return Some(Statement::CollectionOnly);
        }
        if phrase_matches(rules.language, rules.worldwide, clause) {
            return Some(Statement::Worldwide);
        }
        if phrase_matches(rules.language, rules.on_request, clause) {
            return Some(Statement::Price(ShippingPrice::OnRequest(None)));
        }
        if phrase_matches(rules.language, rules.calculated, clause) {
            return Some(Statement::Price(ShippingPrice::ExternallyCalculated(None)));
        }
    }
    for pattern in dispatch {
        if let Some(captures) = pattern.captures(clause) {
            let minimum = captures.name("minimum")?.as_str().parse::<u32>().ok()?;
            let maximum = captures
                .name("maximum")
                .and_then(|v| v.as_str().parse::<u32>().ok())
                .unwrap_or(minimum);
            return Some(
                match TimeRange::new(
                    minimum,
                    maximum,
                    TimeUnit::Days,
                    Some(TimeBasis::Business),
                    None,
                ) {
                    Ok(range) => Statement::Handling(range),
                    Err(_) => Statement::Invalid,
                },
            );
        }
    }
    None
}

fn phrase_matches(language: Language, phrases: &str, clause: &str) -> bool {
    if language == Language::Tr {
        // Default Unicode lowercasing loses Turkish's I/ı versus İ/i distinction.
        // Tolerate that case ambiguity only inside these complete Turkish statements.
        let fold = |c| if c == 'ı' { 'i' } else { c };
        phrases
            .lines()
            .any(|phrase| clause.chars().map(fold).eq(phrase.chars().map(fold)))
    } else {
        phrases.lines().any(|phrase| phrase == clause)
    }
}

fn delivery_related(text: &str) -> bool {
    LANGUAGES.iter().any(|rules| {
        rules
            .delivery_markers
            .iter()
            .any(|marker| text.contains(marker))
            || text.split(|c: char| !c.is_alphabetic()).any(|word| {
                rules
                    .delivery_words
                    .iter()
                    .any(|prefix| word.starts_with(prefix))
            })
    })
}

fn conditional(text: &str) -> bool {
    LANGUAGES.iter().any(|rules| {
        rules
            .condition_markers
            .iter()
            .any(|marker| text.contains(marker))
            || rules.conditions.iter().any(|condition| {
                text.match_indices(condition).any(|(index, matched)| {
                    // Conditions may follow parentheses, labels or other prose. Preserve
                    // word boundaries so ordinary words such as "butler" do not veto.
                    !text[..index].ends_with(char::is_alphabetic)
                        && !text[index + matched.len()..].starts_with(char::is_alphabetic)
                })
            })
    })
}

#[cfg(test)]
mod multilingual_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_english_and_german_item_prohibitions() {
        for text in [
            "Collection only. Shipping is not available for this item.",
            "  SHIPPING IS NOT AVAILABLE FOR THIS ITEM! ",
            "This item is collection only.",
            "This item is pickup only.",
            "Kein Versand für diesen Artikel.",
            "Dieser Artikel ist nur zur Selbstabholung verfügbar.",
        ] {
            assert!(
                matches!(quick_check_shipping(&[text]), ShippingQuickCheck::Resolved(a) if a.prohibited),
                "{text}"
            );
        }
    }

    #[test]
    fn dispatch_is_handling_without_invented_trigger_or_destination() {
        for text in [
            "Dispatch in 10–15 business days",
            "Dispatch in 10 - 15 working days",
            "Versandfertig in 10 bis 15 Werktagen",
        ] {
            let ShippingQuickCheck::Resolved(a) = quick_check_shipping(&[text]) else {
                panic!("{text}");
            };
            let range = a.handling.unwrap();
            assert_eq!(
                (10, 15, TimeUnit::Days, Some(TimeBasis::Business), None),
                (
                    range.minimum(),
                    range.maximum(),
                    range.unit(),
                    range.basis(),
                    range.trigger()
                )
            );
            assert!(!a.worldwide && !a.offered);
        }
    }

    #[test]
    fn pricing_does_not_establish_eligibility() {
        for text in [
            "Shipping costs for this item worldwide on request.",
            "Weltweite Versandkosten für diesen Artikel auf Anfrage.",
            "Shipping costs for this item worldwide are calculated at checkout.",
            "Weltweite Versandkosten für diesen Artikel werden an der Kasse berechnet.",
        ] {
            assert!(
                matches!(quick_check_shipping(&[text]), ShippingQuickCheck::Resolved(a) if a.price.is_some() && !a.offered),
                "{text}"
            );
        }
    }

    #[test]
    fn contradictions_conditions_boilerplate_and_missing_scope_abstain() {
        for text in [
            "Shipping EUR 80",
            "Shipping is not available for this item?",
            "Shipping is not available for this item. Unless arranged.",
            "Collection only",
            "Shipping on request",
            "Delivery in 10–15 business days",
            "Our shipping policy: collection only",
            "All items ship worldwide",
            "See shipping policy",
            "If shipping is not available for this item, contact us",
            "Shipping is not available for this item except by arrangement",
            "Shipping is not available for this item. Shipping for this item is available worldwide.",
            "Shipping is not available for this item. Dispatch in 10–15 business days.",
            "Dispatch in 15–10 business days",
            "Dispatch in 2 days. Dispatch in 10 business days",
            "Shipping costs for this item worldwide on request. Shipping costs for this item worldwide are calculated at checkout.",
        ] {
            assert_eq!(
                ShippingQuickCheck::Unsupported,
                quick_check_shipping(&[text]),
                "{text}"
            );
        }
        assert_eq!(
            ShippingQuickCheck::Unsupported,
            quick_check_shipping(&["This item is pickup only", "Shipping EUR 80"])
        );
        assert_eq!(
            ShippingQuickCheck::Unsupported,
            quick_check_shipping(&[&"x".repeat(MAX_SHIPPING_TEXT_BYTES + 1)])
        );
        assert_eq!(
            ShippingQuickCheck::Unsupported,
            quick_check_shipping(&[""; MAX_SHIPPING_TEXT_FRAGMENTS + 1])
        );
        assert_eq!(
            ShippingQuickCheck::Unsupported,
            quick_check_shipping(&["\0"])
        );
    }

    #[test]
    fn unknown_and_missing_text_produce_no_assertion() {
        for fragments in [
            &[][..],
            &[""][..],
            &["Antique oak chair", "Made in France"][..],
            &["Traditional craftsmanship from a private collection"][..],
        ] {
            assert_eq!(
                ShippingQuickCheck::NoAssertion,
                quick_check_shipping(fragments)
            );
        }
        assert_eq!(
            ShippingQuickCheck::Unsupported,
            quick_check_shipping_with_context(
                &[
                    "Shipping for this item is available worldwide",
                    "Only within Europe"
                ],
                &["Traditional craftsmanship from a private collection"],
            )
        );
        assert!(matches!(
            quick_check_shipping_with_context(
                &["Shipping for this item is available worldwide"],
                &["Traditional craftsmanship from a private collection"],
            ),
            ShippingQuickCheck::Resolved(_)
        ));
    }
}
