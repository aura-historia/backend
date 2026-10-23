use super::*;
use crate::partner_product_listings::UpdateProductListingData;
use listing_source_core::ListingSourceId;
use serde_json::json;

#[test]
fn should_share_the_exact_explicit_key_grammar_with_its_schema() {
    let schema = optional_idempotency_key_schema(&mut schemars::SchemaGenerator::default());
    let validator = jsonschema::validator_for(schema.as_value()).unwrap();
    for byte in 0..=127 {
        let value = String::from_utf8(vec![byte]).unwrap();
        let expected = (0x21..=0x7e).contains(&byte) && byte != b',';
        assert_eq!(
            expected,
            parse_idempotency_key(&value).is_ok(),
            "byte {byte}"
        );
        assert_eq!(
            expected,
            validator.is_valid(&json!(value)),
            "schema byte {byte}"
        );
    }
    for (value, expected) in [
        ("".to_owned(), false),
        ("a".repeat(128), true),
        ("a".repeat(129), false),
        ("é".into(), false),
    ] {
        assert_eq!(expected, parse_idempotency_key(&value).is_ok());
        assert_eq!(expected, validator.is_valid(&json!(value)));
    }
}
#[test]
fn should_preserve_duplicate_fields_for_per_item_rejection_and_original_indices() {
    let source = ListingSourceId::new();
    let raw=parse_envelope(r#"[{"sourceListingId":"first"},{"sourceListingId":"duplicate","price":null,"price":null},{"sourceListingId":"last"}]"#).unwrap();
    let (items, ids, failures) = parse_items::<UpdateProductListingData>(
        raw,
        |data| &data.source_listing_id,
        |data| {
            data.into_key_and_command(source)
                .map(
                    |(product_key, command)| ProductListingIngestionIntent::Update {
                        product_key,
                        command,
                    },
                )
        },
    );
    assert_eq!(
        vec![0, 2],
        items.iter().map(|item| item.index).collect::<Vec<_>>()
    );
    assert_eq!(vec![Some("first".into()), None, Some("last".into())], ids);
    assert_eq!(1, failures[0].index);
    for body in [
        "",
        "{}",
        "[{},",
        "[{},]",
        "[]extra",
        &format!("[{}]", vec!["null"; 101].join(",")),
    ] {
        assert!(parse_envelope(body).is_err());
    }
}
#[test]
fn should_report_missing_outcomes_as_unknown_and_omit_private_classification() {
    use product_listing_service::use_cases::{
        ProductListingIngestionCommandId, ProductListingIngestionSubmissionId,
    };
    let report = report(
        ProductListingIngestionSubmissionResult {
            submission_id: ProductListingIngestionSubmissionId::from_wire(&format!(
                "plis1_{}",
                "a".repeat(64)
            ))
            .unwrap(),
            idempotency_key: parse_idempotency_key("effective").unwrap(),
            original_input_count: 3,
            items: vec![ProductListingIngestionItemOutcome {
                index: 0,
                command_id: ProductListingIngestionCommandId::from_wire(&format!(
                    "plic1_{}",
                    "b".repeat(64)
                ))
                .unwrap(),
                outcome: ProductListingIngestionOutcome::Accepted,
            }],
        },
        vec![BatchFailure::invalid(1, None, "BAD_BODY_VALUE")],
        &[Some("first".into()), None, Some("last".into())],
    );
    let value = serde_json::to_value(report).unwrap();
    assert_eq!(1, value["acceptedCount"]);
    assert_eq!(3, value["originalInputCount"]);
    assert_eq!("effective", value["idempotencyKey"]);
    assert_eq!("UNKNOWN", value["items"][2]["outcome"]);
    assert_eq!(true, value["items"][2]["retryable"]);
    assert!(value["failures"][0].get("kind").is_none());
    let schema = schemars::generate::SchemaSettings::default()
        .for_serialize()
        .into_generator()
        .into_root_schema_for::<AdmissionReportData>();
    assert!(
        jsonschema::validator_for(schema.as_value())
            .unwrap()
            .is_valid(&value)
    );
}
