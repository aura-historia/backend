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
    Unsupported(ShippingUnsupportedReason),
}

/// Why item shipping evidence produced no assertion. Conservative rules are unchanged: every
/// reason abstains. `RulesUnavailable` is a normalizer system failure, not a candidate-data gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShippingUnsupportedReason {
    TooManyFragments {
        observed: usize,
        limit: usize,
    },
    TextTooLarge {
        observed: usize,
        limit: usize,
    },
    /// Questions and embedded NULs are never statements.
    UnsupportedCharacter,
    ConflictingPrice,
    ConflictingHandling,
    InvalidHandlingRange {
        minimum: u32,
        maximum: u32,
    },
    /// A dedicated shipping field contains a clause outside the explicit statement set.
    UnrecognizedFieldClause,
    /// Description prose mentions delivery without a recognized statement.
    UnrecognizedDeliveryClause,
    ConditionalClause,
    ContradictoryAssertions,
    RulesUnavailable,
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
        return ShippingQuickCheck::Unsupported(ShippingUnsupportedReason::TooManyFragments {
            observed: shipping_fields.len().saturating_add(description.len()),
            limit: MAX_SHIPPING_TEXT_FRAGMENTS,
        });
    }
    let fragments = shipping_fields
        .iter()
        .chain(description)
        .copied()
        .collect::<Vec<_>>();
    check_shipping(&fragments, shipping_fields.len())
}

fn check_shipping(fragments: &[&str], shipping_field_count: usize) -> ShippingQuickCheck {
    use ShippingUnsupportedReason as Reason;
    if fragments.len() > MAX_SHIPPING_TEXT_FRAGMENTS {
        return ShippingQuickCheck::Unsupported(Reason::TooManyFragments {
            observed: fragments.len(),
            limit: MAX_SHIPPING_TEXT_FRAGMENTS,
        });
    }
    let text_bytes = fragments.iter().map(|s| s.len()).sum::<usize>();
    if text_bytes > MAX_SHIPPING_TEXT_BYTES {
        return ShippingQuickCheck::Unsupported(Reason::TextTooLarge {
            observed: text_bytes,
            limit: MAX_SHIPPING_TEXT_BYTES,
        });
    }
    if fragments.iter().any(|s| s.contains(['\0', '?', '？', '؟'])) {
        return ShippingQuickCheck::Unsupported(Reason::UnsupportedCharacter);
    }
    let Ok(dispatch) = DISPATCH.as_ref() else {
        return ShippingQuickCheck::Unsupported(Reason::RulesUnavailable);
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
                        return ShippingQuickCheck::Unsupported(Reason::ConflictingPrice);
                    }
                    assertions.worldwide = true;
                    assertions.price = Some(price);
                }
                Some(Statement::Handling(range)) => {
                    if assertions
                        .handling
                        .is_some_and(|previous| previous != range)
                    {
                        return ShippingQuickCheck::Unsupported(Reason::ConflictingHandling);
                    }
                    assertions.handling = Some(range);
                }
                Some(Statement::Invalid { minimum, maximum }) => {
                    return ShippingQuickCheck::Unsupported(Reason::InvalidHandlingRange {
                        minimum,
                        maximum,
                    });
                }
                None => {
                    // Conditions are checked first: they explain the abstention most precisely.
                    if conditional(&clause) {
                        return ShippingQuickCheck::Unsupported(Reason::ConditionalClause);
                    }
                    if index < shipping_field_count {
                        return ShippingQuickCheck::Unsupported(Reason::UnrecognizedFieldClause);
                    }
                    if delivery_related(&clause) {
                        return ShippingQuickCheck::Unsupported(Reason::UnrecognizedDeliveryClause);
                    }
                    continue;
                }
            }
            found = true;
        }
    }
    if contradictory(&assertions, collection_label) {
        ShippingQuickCheck::Unsupported(Reason::ContradictoryAssertions)
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
    Invalid { minimum: u32, maximum: u32 },
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
                    Err(_) => Statement::Invalid { minimum, maximum },
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
            assert!(
                matches!(
                    quick_check_shipping(&[text]),
                    ShippingQuickCheck::Unsupported(_)
                ),
                "{text}"
            );
        }
        use ShippingUnsupportedReason as Reason;
        for (fragments, reason) in [
            (
                &["This item is pickup only", "Shipping EUR 80"][..],
                Reason::UnrecognizedDeliveryClause,
            ),
            (
                &["If shipping is not available for this item, contact us"][..],
                Reason::ConditionalClause,
            ),
            (
                &["Dispatch in 15–10 business days"][..],
                Reason::InvalidHandlingRange {
                    minimum: 15,
                    maximum: 10,
                },
            ),
            (
                &["Dispatch in 2 business days. Dispatch in 10 business days"][..],
                Reason::ConflictingHandling,
            ),
            (
                &[
                    "Shipping costs for this item worldwide on request. Shipping costs for this item worldwide are calculated at checkout.",
                ][..],
                Reason::ConflictingPrice,
            ),
            (
                &[
                    "Shipping is not available for this item. Shipping for this item is available worldwide.",
                ][..],
                Reason::ContradictoryAssertions,
            ),
            (&["Collection only"][..], Reason::ContradictoryAssertions),
            (&["\0"][..], Reason::UnsupportedCharacter),
            (&["Ships today?"][..], Reason::UnsupportedCharacter),
        ] {
            assert_eq!(
                ShippingQuickCheck::Unsupported(reason),
                quick_check_shipping(fragments),
                "{fragments:?}"
            );
        }
        assert_eq!(
            ShippingQuickCheck::Unsupported(Reason::TextTooLarge {
                observed: MAX_SHIPPING_TEXT_BYTES + 1,
                limit: MAX_SHIPPING_TEXT_BYTES,
            }),
            quick_check_shipping(&[&"x".repeat(MAX_SHIPPING_TEXT_BYTES + 1)])
        );
        assert_eq!(
            ShippingQuickCheck::Unsupported(Reason::TooManyFragments {
                observed: MAX_SHIPPING_TEXT_FRAGMENTS + 1,
                limit: MAX_SHIPPING_TEXT_FRAGMENTS,
            }),
            quick_check_shipping(&[""; MAX_SHIPPING_TEXT_FRAGMENTS + 1])
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
            ShippingQuickCheck::Unsupported(ShippingUnsupportedReason::UnrecognizedFieldClause),
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

#[cfg(test)]
mod multilingual_tests {
    use super::{
        ItemShippingAssertions, MAX_SHIPPING_TEXT_BYTES, ShippingQuickCheck, lexicon::LANGUAGES,
        quick_check_shipping, quick_check_shipping_with_context,
    };
    use localization::Language;
    use serde::Deserialize;
    use shipping_core::{
        pricing::ShippingPrice,
        time::{TimeBasis, TimeRange, TimeUnit},
    };
    use std::collections::HashSet;
    use strum::IntoEnumIterator;

    /// Every abstention is conservative; reason-specific coverage lives beside the parser.
    macro_rules! assert_unsupported {
    ($check:expr $(, $($message:tt)+)?) => {
        assert!(
            matches!($check, ShippingQuickCheck::Unsupported(_))
            $(, $($message)+)?
        )
    };
}

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Fixture {
        language: String,
        prohibited: String,
        collection: String,
        label: String,
        offered: String,
        requested: String,
        calculated: String,
        dispatch: String,
        single: String,
        unscoped_price: String,
        arrival: String,
        boilerplate: String,
        exception: String,
        description: String,
    }

    fn fixtures() -> Vec<Fixture> {
        serde_json::from_str(include_str!("shipping/fixtures/languages.json"))
            .expect("language fixtures")
    }

    fn resolved(text: &str) -> ItemShippingAssertions {
        match quick_check_shipping(&[text]) {
            ShippingQuickCheck::Resolved(assertions) => assertions,
            other => panic!("expected an item assertion for {text:?}, got {other:?}"),
        }
    }

    #[test]
    fn every_catalog_language_has_rules_and_independent_fixtures() {
        let expected = Language::iter().collect::<HashSet<_>>();
        let supported = LANGUAGES
            .iter()
            .map(|rules| rules.language)
            .collect::<Vec<_>>();
        assert_eq!(expected.len(), supported.len(), "duplicate language rules");
        assert_eq!(expected, supported.into_iter().collect());
        let fixtures = fixtures();
        let languages = fixtures
            .iter()
            .map(|fixture| {
                Language::from_code(&fixture.language).expect("canonical ISO language code")
            })
            .collect::<HashSet<_>>();
        assert_eq!(
            expected.len(),
            fixtures.len(),
            "duplicate language fixtures"
        );
        assert_eq!(expected, languages);
    }

    #[test]
    fn item_prohibitions_and_collection_only_are_supported_in_every_language() {
        for fixture in fixtures() {
            for text in [&fixture.prohibited, &fixture.collection] {
                assert_eq!(
                    ItemShippingAssertions {
                        prohibited: true,
                        ..Default::default()
                    },
                    resolved(text),
                    "{}: {text}",
                    fixture.language
                );
            }
            assert_unsupported!(
                quick_check_shipping(&[&fixture.label]),
                "bare label needs item scope: {}",
                fixture.language
            );
            assert_eq!(
                ItemShippingAssertions {
                    prohibited: true,
                    ..Default::default()
                },
                resolved(&format!("{}。{}", fixture.label, fixture.prohibited)),
                "{}",
                fixture.language
            );
        }
    }

    #[test]
    fn worldwide_scope_and_pricing_do_not_invent_eligibility_currency_or_basis() {
        for fixture in fixtures() {
            assert_eq!(
                ItemShippingAssertions {
                    worldwide: true,
                    offered: true,
                    ..Default::default()
                },
                resolved(&fixture.offered),
                "{}",
                fixture.language
            );
            for (text, price) in [
                (&fixture.requested, ShippingPrice::OnRequest(None)),
                (
                    &fixture.calculated,
                    ShippingPrice::ExternallyCalculated(None),
                ),
            ] {
                assert_eq!(
                    ItemShippingAssertions {
                        worldwide: true,
                        price: Some(price),
                        ..Default::default()
                    },
                    resolved(text),
                    "{}: {text}",
                    fixture.language
                );
            }
        }
    }

    #[test]
    fn localized_dispatch_is_handling_with_original_units_and_no_trigger() {
        for fixture in fixtures() {
            for (text, maximum) in [(&fixture.dispatch, 15), (&fixture.single, 10)] {
                let expected =
                    TimeRange::new(10, maximum, TimeUnit::Days, Some(TimeBasis::Business), None)
                        .unwrap();
                assert_eq!(
                    ItemShippingAssertions {
                        handling: Some(expected),
                        ..Default::default()
                    },
                    resolved(text),
                    "{}: {text}",
                    fixture.language
                );
            }
            let combined = resolved(&format!("{}؛{}", fixture.requested, fixture.dispatch));
            assert!(
                combined.worldwide && !combined.offered,
                "{}",
                fixture.language
            );
            assert_eq!(Some(ShippingPrice::OnRequest(None)), combined.price);
            assert_eq!(15, combined.handling.unwrap().maximum());
            assert_eq!(None, combined.handling.unwrap().trigger());
        }
    }

    #[test]
    fn unknown_delivery_arrivals_conditions_and_boilerplate_abstain_in_every_language() {
        for fixture in fixtures() {
            for text in [
                &fixture.unscoped_price,
                &fixture.arrival,
                &fixture.boilerplate,
                &fixture.exception,
            ] {
                assert_unsupported!(
                    quick_check_shipping(&[text]),
                    "{}: {text}",
                    fixture.language
                );
                assert_unsupported!(
                    quick_check_shipping_with_context(&[&fixture.offered], &[text]),
                    "contradictory description in {}: {text}",
                    fixture.language
                );
            }
            for text in [
                format!("{} {}", fixture.exception, fixture.offered),
                format!(
                    "{} {}",
                    fixture.offered.trim_end_matches(['.', '。']),
                    fixture.exception
                ),
            ] {
                assert_unsupported!(
                    quick_check_shipping(&[&text]),
                    "{}: {text}",
                    fixture.language
                );
            }
            assert_eq!(
                ShippingQuickCheck::NoAssertion,
                quick_check_shipping(&[&fixture.description]),
                "ordinary description: {}",
                fixture.language
            );
            assert!(
                matches!(
                    quick_check_shipping_with_context(&[&fixture.offered], &[&fixture.description]),
                    ShippingQuickCheck::Resolved(_)
                ),
                "ordinary description must not hide a valid assertion: {}",
                fixture.language
            );
        }
    }

    #[test]
    fn contradictions_remain_atomic_including_mixed_languages_and_scripts() {
        let fixtures = fixtures();
        for fixture in &fixtures {
            for (left, right) in [
                (&fixture.prohibited, &fixture.offered),
                (&fixture.prohibited, &fixture.requested),
                (&fixture.prohibited, &fixture.dispatch),
                (&fixture.requested, &fixture.calculated),
                (&fixture.dispatch, &fixture.single),
            ] {
                assert_unsupported!(
                    quick_check_shipping(&[left, right]),
                    "{}: {left} / {right}",
                    fixture.language
                );
                assert_unsupported!(
                    quick_check_shipping(&[right, left]),
                    "{}: {right} / {left}",
                    fixture.language
                );
            }
            for other in &fixtures {
                assert_unsupported!(
                    quick_check_shipping_with_context(&[&fixture.prohibited], &[&other.offered]),
                    "{} prohibition / {} offer",
                    fixture.language,
                    other.language
                );
                assert_unsupported!(
                    quick_check_shipping_with_context(&[&fixture.offered], &[&other.exception]),
                    "{} offer / {} condition",
                    fixture.language,
                    other.language
                );
                assert_unsupported!(
                    quick_check_shipping_with_context(&[&fixture.offered, &other.arrival], &[]),
                    "{} offer / {} arrival",
                    fixture.language,
                    other.language
                );
            }
        }
    }

    #[test]
    fn descriptions_preserve_parenthesized_and_prefaced_conditions() {
        for fixture in fixtures() {
            for context in [
                format!("({})", fixture.exception),
                format!("Note: {}", fixture.exception),
            ] {
                assert_unsupported!(
                    quick_check_shipping_with_context(&[&fixture.offered], &[&context]),
                    "{}: {context}",
                    fixture.language
                );
            }
        }
        assert_eq!(
            ShippingQuickCheck::NoAssertion,
            quick_check_shipping(&["A portrait of a butler from Butley in a shell frame."])
        );
    }

    #[test]
    fn every_documented_exact_phrase_supports_case_and_layout_variations() {
        for rules in LANGUAGES {
            for (phrases, expected) in [
                (
                    rules.prohibited,
                    ItemShippingAssertions {
                        prohibited: true,
                        ..Default::default()
                    },
                ),
                (
                    rules.worldwide,
                    ItemShippingAssertions {
                        worldwide: true,
                        offered: true,
                        ..Default::default()
                    },
                ),
                (
                    rules.on_request,
                    ItemShippingAssertions {
                        worldwide: true,
                        price: Some(ShippingPrice::OnRequest(None)),
                        ..Default::default()
                    },
                ),
                (
                    rules.calculated,
                    ItemShippingAssertions {
                        worldwide: true,
                        price: Some(ShippingPrice::ExternallyCalculated(None)),
                        ..Default::default()
                    },
                ),
            ] {
                for phrase in phrases.lines() {
                    assert_eq!(expected, resolved(phrase), "{:?}: {phrase}", rules.language);
                    let decorated =
                        format!("  {}！  ", phrase.to_uppercase().replace(' ', "\u{a0}  "));
                    assert_eq!(
                        expected,
                        resolved(&decorated),
                        "{:?}: {decorated}",
                        rules.language
                    );
                }
            }
            for label in rules.collection_only.lines() {
                assert_unsupported!(
                    quick_check_shipping(&[label]),
                    "{:?}: {label}",
                    rules.language
                );
                assert!(
                    resolved(&format!(
                        "{}؛{}",
                        label,
                        rules.prohibited.lines().next().unwrap()
                    ))
                    .prohibited
                );
            }
        }
    }

    #[test]
    fn unicode_questions_range_errors_and_missing_dispatch_verbs_fail_closed() {
        for fixture in fixtures() {
            for question in ["?", "？", "؟"] {
                let question = format!("{}{}", fixture.prohibited, question);
                assert_unsupported!(quick_check_shipping(&[&question]), "{}", fixture.language);
            }
        }
        for text in [
            "この商品は10営業日",
            "本商品は10営業日以内に届きます",
            "本商品将在15至10个工作日内发货",
            "この商品は１５〜１０営業日以内に発送します",
            "يتم إرسال هذا المنتج خلال ١٥ إلى ١٠ أيام عمل",
            "Expédition sous 15 à 10 jours ouvrés",
            "Envío en 15 a 10 días hábiles",
            "Spedizione entro 15-10 giorni lavorativi",
            "Expedição em 15 a 10 dias úteis",
            "Wysyłka w ciągu 15-10 dni roboczych",
            "Bu ürün 15 ila 10 iş günü içinde kargoya verilir",
            "Verzending binnen 15 tot 10 werkdagen",
            "Odeslání do 15 až 10 pracovních dnů",
            "Отправка в течение 15-10 рабочих дней",
            "يتم إرسال هذا المنتج خلال ١٠٠٠ أيام عمل",
            "本商品将在十至十五个工作日内发货",
            "この商品は十から十五営業日以内に発送します",
            "يتم إرسال هذا المنتج خلال عشرة أيام عمل",
            "Dit artikel wordt binnen 10 werkdagen",
            "Dieser Artikel wird innerhalb von 10 Werktagen",
        ] {
            assert!(
                !matches!(
                    quick_check_shipping(&[text]),
                    ShippingQuickCheck::Resolved(_)
                ),
                "{text}"
            );
        }
        assert!(matches!(
            quick_check_shipping(&["Dispatch in 1 business day"]),
            ShippingQuickCheck::Resolved(_)
        ));
        assert!(matches!(
            quick_check_shipping(&["Spedizione entro 1 giorno lavorativo"]),
            ShippingQuickCheck::Resolved(_)
        ));
        assert!(matches!(
            quick_check_shipping(&["Odeslání do 1 pracovního dne"]),
            ShippingQuickCheck::Resolved(_)
        ));
        for text in [
            "Wysyłka w ciągu 1 dnia roboczego",
            "Отправка в течение 1 рабочего дня",
        ] {
            assert!(
                matches!(
                    quick_check_shipping(&[text]),
                    ShippingQuickCheck::Resolved(_)
                ),
                "{text}"
            );
        }
    }

    #[test]
    fn unicode_layout_and_byte_bounds_do_not_change_scope_or_numeric_values() {
        assert_unsupported!(quick_check_shipping(&[
            "Shipping\u{307} is not available for this item"
        ]));
        let ordinary = "يتم إرسال هذا المنتج خلال 10-15 أيام عمل";
        let decorated = "\u{200f}يتم إرسال هذا المنتج خلال ١٠–۱۵ أيام عمل\u{200e}";
        assert_eq!(resolved(ordinary), resolved(decorated));
        let mut boundary = "هذا المنتج متاح للاستلام الشخصي فقط".to_owned();
        boundary.extend(std::iter::repeat_n(
            ' ',
            MAX_SHIPPING_TEXT_BYTES - boundary.len(),
        ));
        assert!(resolved(&boundary).prohibited);
        boundary.push(' ');
        assert_unsupported!(quick_check_shipping(&[&boundary]));
        assert_unsupported!(quick_check_shipping(&[
            &"商".repeat(MAX_SHIPPING_TEXT_BYTES / 3 + 1)
        ]));
    }
}
