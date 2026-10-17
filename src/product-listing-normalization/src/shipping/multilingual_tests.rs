use super::*;
use serde::Deserialize;
use std::collections::HashSet;
use strum::IntoEnumIterator;

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
    serde_json::from_str(include_str!("fixtures/languages.json")).expect("language fixtures")
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
        .map(|fixture| Language::from_code(&fixture.language).expect("canonical ISO language code"))
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
        assert_eq!(
            ShippingQuickCheck::Unsupported,
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
            assert_eq!(
                ShippingQuickCheck::Unsupported,
                quick_check_shipping(&[text]),
                "{}: {text}",
                fixture.language
            );
            assert_eq!(
                ShippingQuickCheck::Unsupported,
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
            assert_eq!(
                ShippingQuickCheck::Unsupported,
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
            assert_eq!(
                ShippingQuickCheck::Unsupported,
                quick_check_shipping(&[left, right]),
                "{}: {left} / {right}",
                fixture.language
            );
            assert_eq!(
                ShippingQuickCheck::Unsupported,
                quick_check_shipping(&[right, left]),
                "{}: {right} / {left}",
                fixture.language
            );
        }
        for other in &fixtures {
            assert_eq!(
                ShippingQuickCheck::Unsupported,
                quick_check_shipping_with_context(&[&fixture.prohibited], &[&other.offered]),
                "{} prohibition / {} offer",
                fixture.language,
                other.language
            );
            assert_eq!(
                ShippingQuickCheck::Unsupported,
                quick_check_shipping_with_context(&[&fixture.offered], &[&other.exception]),
                "{} offer / {} condition",
                fixture.language,
                other.language
            );
            assert_eq!(
                ShippingQuickCheck::Unsupported,
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
            assert_eq!(
                ShippingQuickCheck::Unsupported,
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
            for phrase in phrases {
                assert_eq!(expected, resolved(phrase), "{:?}: {phrase}", rules.language);
                let decorated = format!("  {}！  ", phrase.to_uppercase().replace(' ', "\u{a0}  "));
                assert_eq!(
                    expected,
                    resolved(&decorated),
                    "{:?}: {decorated}",
                    rules.language
                );
            }
        }
        for label in rules.collection_only {
            assert_eq!(
                ShippingQuickCheck::Unsupported,
                quick_check_shipping(&[label]),
                "{:?}: {label}",
                rules.language
            );
            assert!(resolved(&format!("{}؛{}", label, rules.prohibited[0])).prohibited);
        }
    }
}

#[test]
fn unicode_questions_range_errors_and_missing_dispatch_verbs_fail_closed() {
    for fixture in fixtures() {
        for question in ["?", "？", "؟"] {
            let question = format!("{}{}", fixture.prohibited, question);
            assert_eq!(
                ShippingQuickCheck::Unsupported,
                quick_check_shipping(&[&question]),
                "{}",
                fixture.language
            );
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
    assert_eq!(
        ShippingQuickCheck::Unsupported,
        quick_check_shipping(&["Shipping\u{307} is not available for this item"])
    );
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
    assert_eq!(
        ShippingQuickCheck::Unsupported,
        quick_check_shipping(&[&boundary])
    );
    assert_eq!(
        ShippingQuickCheck::Unsupported,
        quick_check_shipping(&[&"商".repeat(MAX_SHIPPING_TEXT_BYTES / 3 + 1)])
    );
}
