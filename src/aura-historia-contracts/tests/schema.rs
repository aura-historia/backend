use application::patch_field::PatchField;
use aura_historia_contracts::{
    patch_value::{PatchValue, clearable},
    values::{LocalizedTextData, PriceData, ProductListingPriceData},
};
use localization::Language;
use money::Currency;
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use strum::IntoEnumIterator;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/common-values.json")).expect("valid fixture")
}

fn validator<T: JsonSchema>(settings: SchemaSettings) -> jsonschema::Validator {
    let schema = settings.into_generator().into_root_schema_for::<T>();
    jsonschema::validator_for(schema.as_value()).expect("valid generated schema")
}

fn assert_input_agreement<T: DeserializeOwned + JsonSchema>(
    samples: impl IntoIterator<Item = (Value, bool)>,
) {
    let validator = validator::<T>(SchemaSettings::default().for_deserialize());
    for (sample, accepted) in samples {
        assert_eq!(
            accepted,
            serde_json::from_value::<T>(sample.clone()).is_ok(),
            "Serde acceptance of {sample}"
        );
        assert_eq!(
            accepted,
            validator.is_valid(&sample),
            "schema acceptance of {sample}"
        );
    }
}

fn assert_output<T: Serialize + JsonSchema>(value: &T) -> Value {
    let serialized = serde_json::to_value(value).expect("serialize contract value");
    let validator = validator::<T>(SchemaSettings::default().for_serialize());
    assert!(
        validator.is_valid(&serialized),
        "output schema must accept actual JSON: {serialized}"
    );
    serialized
}

#[test]
fn should_describe_every_currency_and_unsigned_minor_unit_amount() {
    let mut samples: Vec<_> = Currency::iter()
        .map(|currency| (json!({"currency": currency.as_str(), "amount": 1}), true))
        .collect();
    samples.extend([
        (fixture()["price"].clone(), true),
        (json!({"currency": "EUR", "amount": 0}), true),
        (json!({"currency": "EUR", "amount": u64::MAX}), true),
        (json!({"currency": "EUR", "amount": 1, "extra": true}), true),
        (json!({"currency": "eur", "amount": 1}), false),
        (json!({"currency": "XXX", "amount": 1}), false),
        (json!({"currency": "EUR", "amount": -1}), false),
        (json!({"currency": "EUR", "amount": 1.5}), false),
        (
            json!({"currency": "EUR", "amount": 18_446_744_073_709_551_616_f64}),
            false,
        ),
        (json!({"currency": "EUR", "amount": "1"}), false),
        (json!({"currency": "EUR", "amount": null}), false),
        (json!({"currency": "EUR"}), false),
        (json!({"amount": 1}), false),
        (json!(null), false),
    ]);
    assert_input_agreement::<PriceData>(samples);

    for currency in Currency::iter() {
        assert_output(&PriceData {
            currency,
            amount: 1,
        });
    }
    assert_output(&PriceData {
        currency: Currency::Eur,
        amount: u64::MAX,
    });
}

#[test]
fn should_describe_language_alias_input_and_canonical_output() {
    let mut samples: Vec<_> = Language::iter()
        .map(|language| {
            (
                json!({"text": "Cabinet", "language": language.as_str()}),
                true,
            )
        })
        .collect();
    samples.extend([
        (fixture()["localizedTextInput"].clone(), true),
        (json!({"text": "x", "language": "de-CH"}), true),
        (json!({"text": "x", "language": "en_IE"}), true),
        (json!({"text": "x", "language": "en-IE"}), false),
        (json!({"text": "x", "language": "EN"}), false),
        (json!({"text": "x", "language": "xx"}), false),
        (json!({"text": "x", "language": 1}), false),
        (json!({"text": null, "language": "en"}), false),
        (json!({"text": "x"}), false),
        (json!({"language": "en"}), false),
    ]);
    assert_input_agreement::<LocalizedTextData>(samples);

    let input = fixture()["localizedTextInput"].clone();
    let text: LocalizedTextData = serde_json::from_value(input.clone()).expect("valid text");
    assert_eq!(fixture()["localizedTextOutput"], assert_output(&text));
    let output = validator::<LocalizedTextData>(SchemaSettings::default().for_serialize());
    assert!(!output.is_valid(&input), "locale alias is input-only");
    for language in Language::iter() {
        assert_output(&LocalizedTextData {
            text: "Cabinet".to_owned(),
            language,
        });
    }
}

#[test]
fn should_accept_each_advertised_language_alias_in_serde() {
    let schema = SchemaSettings::default()
        .for_deserialize()
        .into_generator()
        .into_root_schema_for::<LocalizedTextData>();
    let codes = schema.as_value()["properties"]["language"]["enum"]
        .as_array()
        .expect("language schema enum");
    assert_eq!(14 + 39, codes.len());
    for code in codes {
        let input = json!({"text": "Cabinet", "language": code});
        let value: LocalizedTextData =
            serde_json::from_value(input).expect("advertised code accepted");
        assert_output(&value);
    }
}

#[test]
fn should_describe_tagged_prices_and_existing_unknown_member_behavior() {
    let mut samples: Vec<_> = fixture()["listingPrices"]
        .as_array()
        .expect("listing prices")
        .iter()
        .cloned()
        .map(|sample| (sample, true))
        .collect();
    samples.extend([
        (
            json!({"type": "MONETARY", "currency": "USD", "amount": u64::MAX}),
            true,
        ),
        // The existing unit variant ignores extra members; the struct variant rejects them.
        (
            json!({"type": "ON_REQUEST", "currency": "XXX", "amount": null}),
            true,
        ),
        (
            json!({"type": "MONETARY", "currency": "EUR", "amount": 1, "extra": true}),
            false,
        ),
        (
            json!({"type": "MONETARY", "currency": "XXX", "amount": 1}),
            false,
        ),
        (json!({"type": "MONETARY", "amount": 1}), false),
        (
            json!({"type": "MONETARY", "currency": "EUR", "amount": -1}),
            false,
        ),
        (
            json!({"type": "monetary", "currency": "EUR", "amount": 1}),
            false,
        ),
        (json!({"type": "UNKNOWN"}), false),
        (json!({}), false),
        (json!(null), false),
    ]);
    assert_input_agreement::<ProductListingPriceData>(samples);
    assert_eq!(
        json!({"type": "ON_REQUEST"}),
        assert_output(&ProductListingPriceData::OnRequest)
    );
    assert_output(&ProductListingPriceData::Monetary {
        currency: Currency::Jpy,
        amount: 1_200,
    });
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PricePatch {
    #[serde(default)]
    value: PatchValue<PriceData>,
}

#[test]
fn should_describe_patch_omission_null_and_value_without_enum_wrappers() {
    let fixture = fixture();
    for name in ["omitted", "null", "value"] {
        assert_input_agreement::<PricePatch>([(fixture["patches"][name].clone(), true)]);
    }
    assert_input_agreement::<PricePatch>([
        (json!({"value": {"currency": "XXX", "amount": 1}}), false),
        (json!({"value": {"Omitted": null}}), false),
        (json!({"value": "Omitted"}), false),
    ]);
    let omitted: PricePatch =
        serde_json::from_value(fixture["patches"]["omitted"].clone()).unwrap();
    let null: PricePatch = serde_json::from_value(fixture["patches"]["null"].clone()).unwrap();
    let value: PricePatch = serde_json::from_value(fixture["patches"]["value"].clone()).unwrap();
    assert_eq!(PatchField::Unchanged, clearable(omitted.value));
    assert_eq!(PatchField::Clear, clearable(null.value));
    assert_eq!(
        PatchField::Set(PriceData {
            currency: Currency::Chf,
            amount: 1_200
        }),
        clearable(value.value)
    );
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PricePatchWithoutDefault {
    value: PatchValue<PriceData>,
}

#[test]
fn should_describe_implicit_null_when_patch_member_has_no_default() {
    assert_input_agreement::<PricePatchWithoutDefault>([
        (json!({}), true),
        (json!({"value": null}), true),
        (json!({"value": fixture()["price"]}), true),
        (json!({"value": {"currency": "XXX", "amount": 1}}), false),
    ]);
    let omitted: PricePatchWithoutDefault = serde_json::from_value(json!({})).unwrap();
    assert_eq!(PatchValue::Null, omitted.value);
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct OptionalPrice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    optional_price: Option<PriceData>,
}

#[test]
fn should_describe_optional_nullable_values_and_omitted_output() {
    assert_input_agreement::<OptionalPrice>([
        (json!({}), true),
        (json!({"optionalPrice": null}), true),
        (json!({"optionalPrice": fixture()["price"]}), true),
        (json!({"optionalPrice": "CHF"}), false),
    ]);
    assert_eq!(
        json!({}),
        assert_output(&OptionalPrice {
            optional_price: None
        })
    );
    assert_output(&OptionalPrice {
        optional_price: Some(PriceData {
            currency: Currency::Chf,
            amount: 1_200,
        }),
    });
}
