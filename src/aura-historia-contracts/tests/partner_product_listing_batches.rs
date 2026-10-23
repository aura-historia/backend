use application::patch_field::PatchField;
use aura_historia_contracts::partner_product_listings::{
    CreateProductListingData, PartnerProductListingInput, UpdateProductListingData,
    UpsertProductListingData, WithdrawProductListingData, prepare_batch,
};
use listing_source_core::ListingSourceId;
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn assert_input<T: JsonSchema + DeserializeOwned + PartnerProductListingInput>(
    body: Value,
    allowed: bool,
) {
    let schema = SchemaSettings::default()
        .for_deserialize()
        .into_generator()
        .into_root_schema_for::<T>();
    let validator = jsonschema::options()
        .should_validate_formats(true)
        .build(schema.as_value())
        .unwrap();
    assert_eq!(allowed, validator.is_valid(&body), "schema: {body}");
    let mapped = serde_json::from_value::<T>(body.clone())
        .ok()
        .and_then(|item| item.into_batch_command(ListingSourceId::new()).ok());
    assert_eq!(allowed, mapped.is_some(), "mapping: {body}");
}

#[test]
fn should_preserve_shared_patch_schemas_and_wire_compatibility() {
    for (body, allowed) in [
        (json!({"sourceListingId":"sku"}), true),
        (
            json!({"sourceListingId":"sku","price":null,"priceEstimateMin":null,"priceEstimateMax":null,"availability":null,"images":[]}),
            true,
        ),
        (
            json!({"sourceListingId":"sku","price":{"type":"ON_REQUEST","legacy":true},"auction":{"auctionId":null,"lotNumber":null,"cataloguePosition":u32::MAX,"timing":{"biddingOpens":null,"scheduledCloses":"2026-10-23T12:00:00Z","reportedClosedAt":null}}}),
            true,
        ),
        (
            json!({"sourceListingId":"sku","price":{"type":"MONETARY","currency":"EUR","amount":u64::MAX},"priceEstimateMin":{"currency":"USD","amount":0},"priceEstimateMax":{"currency":"GBP","amount":u64::MAX}}),
            true,
        ),
        (json!({"sourceListingId":"sku","url":null}), false),
        (json!({"sourceListingId":"sku","images":null}), false),
        (json!({"sourceListingId":"sku","auction":null}), false),
        (
            json!({"sourceListingId":"sku","availability":"WITHDRAWN"}),
            false,
        ),
        (
            json!({"sourceListingId":"sku","title":{"text":"read only","language":"en"}}),
            false,
        ),
        (
            json!({"sourceListingId":"sku","auction":{"cataloguePosition":0}}),
            false,
        ),
        (
            json!({"sourceListingId":"sku","auction":{"cataloguePosition":u64::MAX}}),
            false,
        ),
        (
            json!({"sourceListingId":"sku","auction":{"timing":{"scheduledCloses":"2026-10-23"}}}),
            false,
        ),
        (
            json!({"sourceListingId":"sku","auction":{"sourceAuctionId":"retired"}}),
            false,
        ),
    ] {
        assert_input::<UpdateProductListingData>(body, allowed);
    }
    assert_input::<CreateProductListingData>(
        json!({"sourceListingId":"sku","title":null,"description":null,"url":"https://example.test/listing","images":[]}),
        true,
    );
    assert_input::<CreateProductListingData>(json!({"sourceListingId":"sku","images":[]}), false);
    assert_input::<UpsertProductListingData>(
        json!({"sourceListingId":"sku","url":null,"title":{"text":"Title","language":"de-DE"},"description":{"text":"Description","language":"en"}}),
        true,
    );
    assert_input::<UpsertProductListingData>(json!({"sourceListingId":"sku","images":null}), false);
    assert_input::<WithdrawProductListingData>(
        json!({"sourceListingId":"sku","legacy":true}),
        true,
    );
}

#[test]
fn should_prepare_all_items_before_returning_commands_and_keep_original_identity_spelling() {
    let source = ListingSourceId::new();
    let input: Vec<UpdateProductListingData> = serde_json::from_value(json!([
        {"sourceListingId":"  SKU  #42/Blue  ","price":null,"images":[],"availability":null},
        {"sourceListingId":"SKU  #42/Blue","price":{"type":"ON_REQUEST"}}
    ]))
    .unwrap();
    let prepared = prepare_batch(source, input).unwrap();
    assert_eq!(
        vec!["  SKU  #42/Blue  ", "SKU  #42/Blue"],
        prepared.source_listing_ids
    );
    assert_eq!(prepared.commands[0].0, prepared.commands[1].0);
    assert!(matches!(prepared.commands[0].1.price, PatchField::Clear));
    assert!(
        matches!(prepared.commands[0].1.images, PatchField::Set(ref images) if images.is_empty())
    );
    assert!(matches!(
        prepared.commands[0].1.availability,
        PatchField::Clear
    ));
    assert!(matches!(
        prepared.commands[1].1.images,
        PatchField::Unchanged
    ));
    let invalid: Vec<WithdrawProductListingData> =
        serde_json::from_value(json!([{"sourceListingId":"valid"},{"sourceListingId":" \t"}]))
            .unwrap();
    assert!(prepare_batch(source, invalid).is_err());
    let too_many: Vec<WithdrawProductListingData> =
        serde_json::from_value(json!(vec![json!({"sourceListingId":"same"}); 101])).unwrap();
    assert!(prepare_batch(source, too_many).is_err());
    for count in [0, 100] {
        let inputs: Vec<WithdrawProductListingData> =
            serde_json::from_value(json!(vec![json!({"sourceListingId":"same"}); count])).unwrap();
        assert_eq!(count, prepare_batch(source, inputs).unwrap().commands.len());
    }
}

#[test]
fn should_reject_duplicate_known_json_members_without_changing_withdrawal_unknown_field_compatibility()
 {
    for body in [
        r#"{"sourceListingId":"first","sourceListingId":"second"}"#,
        r#"{"sourceListingId":"same","availability":"AVAILABLE","availability":"SOLD_OUT"}"#,
    ] {
        assert!(serde_json::from_str::<UpdateProductListingData>(body).is_err());
    }
    assert!(
        serde_json::from_str::<WithdrawProductListingData>(
            r#"{"sourceListingId":"first","sourceListingId":"second"}"#
        )
        .is_err()
    );
}
