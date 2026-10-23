use application::patch_field::PatchField;
use aura_historia_contracts::product_listing_delivery::*;
use listing_source_core::ListingSourceId;
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};
use product_listing_core::product_listing_id::{ProductListingId, ProductListingKey};
use product_listing_service::{ports::ProductListingStorageVersion, use_cases::*};
use schemars::{JsonSchema, generate::SchemaSettings};
use serde_json::{Value, json};
use shipping_core::{fixtures::gallery_terms, profile::ShippingProfileId};

fn validator<T: JsonSchema>(output: bool) -> jsonschema::Validator {
    let settings = if output {
        SchemaSettings::default().for_serialize()
    } else {
        SchemaSettings::default().for_deserialize()
    };
    jsonschema::validator_for(
        &serde_json::to_value(settings.into_generator().into_root_schema_for::<T>()).unwrap(),
    )
    .unwrap()
}
fn key() -> ProductListingKey {
    ProductListingKey::new(
        ListingSourceId::new(),
        "opaque/source #42".try_into().unwrap(),
    )
}
fn full() -> Value {
    let mut terms =
        aura_historia_contracts::shipping::terms::encode(&gallery_terms(u64::MAX)).unwrap();
    terms["collection"]["site"] = json!({"kind":"ASSERTED","geography":{"reference_release":"iso-codes-4.20.1","address_text":"Gallery","country":"DE","subdivision":"DE-BE","postal_code":"10115"},"position":{"point":{"latitude_degrees":52.5,"longitude_degrees":13.4},"precision":"PREMISES","accuracy_metres":5.0}});
    terms["shipping"]["coverage"]["dispatch_origin"] =
        json!({"kind":"PARTY_LOCATION","party_location_id":PartyLocationId::new()});
    terms["shipping"]["coverage"]["zones"][0]["services"][0]["arranger"] =
        json!({"kind":"PARTY","party_id":PartyId::new()});
    json!({"expectedVersion":1,"import":{"integration":{"kind":"SOURCE"},"sequence":2,"acquiredAt":"2026-10-15T10:00:00Z","requestKey":"delivery-2","objectKey":"policy-1","observation":{}},"delivery":{"state":"SET","value":{
        "integration":{"kind":"SOURCE"},
        "selection":{"state":"SET","value":{"kind":"INLINE","terms":terms}},
        "rateInputs":{"state":"SET","value":{"classes":{"codes":["fragile","painting"],"completeness":"COMPLETE"},"variants":["gid://synthetic/Variant/2","packed"]}},
        "shipping":{"state":"SET","value":{"kind":"REPLACE","value":terms["shipping"]}},
        "collection":{"state":"SET","value":{"kind":"REPLACE","value":terms["collection"]}}
    }}})
}
fn roundtrip(value: Value) -> ChangeProductListingDeliveryCommand {
    let schema = validator::<CommandData>(false);
    assert!(
        schema.is_valid(&value),
        "{value}: {:?}",
        schema
            .iter_errors(&value)
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
    );
    let command = decode_command(value.clone(), key()).unwrap();
    assert_eq!(encode_command(&command).unwrap(), value);
    command
}
#[test]
fn should_preserve_full_terms_rate_inputs_overrides_imports_and_public_references() {
    let command = roundtrip(full());
    let import = command.import.as_ref().unwrap();
    assert_eq!(import.sequence, 2);
    assert_eq!(import.object_key.as_deref(), Some("policy-1"));
    assert_eq!(
        import.acquired_at,
        import.observation.as_ref().unwrap().acquired_at
    );
    let PatchField::Set(patch) = &command.delivery else {
        panic!("set delivery")
    };
    let delivery = patch
        .apply(command.product_key.listing_source_id, None)
        .unwrap();
    let view = serde_json::to_value(
        DeliveryData::try_from(ProductListingDeliveryView {
            product_listing_id: ProductListingId::new(),
            version: ProductListingStorageVersion::INITIAL,
            delivery: Some(delivery),
        })
        .unwrap(),
    )
    .unwrap();
    assert!(validator::<DeliveryData>(true).is_valid(&view), "{view}");
    assert_eq!(
        view["delivery"]["source_id"],
        json!(command.product_key.listing_source_id)
    );
    assert_eq!(view["delivery"]["selection"]["terms"], full_terms(&command));
    let mut invalid = view;
    invalid["delivery"]["source_id"] = json!(
        command
            .product_key
            .listing_source_id
            .into_uuid()
            .to_string()
    );
    assert!(!validator::<DeliveryData>(true).is_valid(&invalid));
}
fn full_terms(command: &ChangeProductListingDeliveryCommand) -> Value {
    encode_command(command).unwrap()["delivery"]["value"]["selection"]["value"]["terms"].clone()
}
#[test]
fn should_keep_all_patch_selection_override_and_integration_variants_in_the_schema() {
    for state in ["UNCHANGED", "CLEAR"] {
        roundtrip(json!({"expectedVersion":1,"delivery":{"state":state}}));
    }
    for selection in [
        json!({"kind":"INHERIT_SOURCE_DEFAULT"}),
        json!({"kind":"SUPPRESS_INHERITANCE"}),
        json!({"kind":"EXPLICIT","profile_id":ShippingProfileId::new()}),
    ] {
        for integration in [
            json!({"kind":"SOURCE"}),
            json!({"kind":"PROVIDER","method":"SHOPIFY"}),
            json!({"kind":"PROVIDER","method":"WOOCOMMERCE"}),
        ] {
            let mut value = full();
            value.as_object_mut().unwrap().remove("import");
            value["delivery"]["value"]["integration"] = integration;
            value["delivery"]["value"]["selection"]["value"] = selection.clone();
            for field in ["shipping", "collection"] {
                value["delivery"]["value"][field] =
                    json!({"state":"SET","value":{"kind":"INHERIT"}});
            }
            roundtrip(value);
        }
    }
    for state in ["UNCHANGED", "CLEAR"] {
        let mut value = full();
        for field in ["selection", "rateInputs", "shipping", "collection"] {
            value["delivery"]["value"][field] = json!({"state":state});
        }
        roundtrip(value);
    }
    let mut value = full();
    value["delivery"]["value"]["rateInputs"]["value"] = json!({"classes":null,"variants":null});
    value["delivery"]["value"]["shipping"]["value"] = json!({"kind":"CLEAR"});
    value["delivery"]["value"]["collection"]["value"] = json!({"kind":"CLEAR"});
    roundtrip(value);
}
#[test]
fn should_reject_invalid_versions_references_rules_and_required_nullable_omissions() {
    for (path, replacement) in [
        ("/expectedVersion", json!(0)),
        ("/expectedVersion", json!(u64::MAX)),
        ("/delivery/value/integration/method", json!("WEB_CRAWL")),
        (
            "/delivery/value/rateInputs/value/classes/completeness",
            json!("UNKNOWN"),
        ),
        (
            "/delivery/value/selection/value/terms/shipping/coverage/dispatch_origin/party_location_id",
            json!(uuid::Uuid::now_v7().to_string()),
        ),
        (
            "/delivery/value/shipping/value/value/coverage/zones/0/services/0/arranger/party_id",
            json!(uuid::Uuid::now_v7().to_string()),
        ),
        (
            "/delivery/value/collection/value/value/dependency",
            json!("NOT_DEPENDENT"),
        ),
        ("/import/requestKey", json!("with space")),
    ] {
        let mut value = full();
        if path == "/delivery/value/integration/method" {
            value["delivery"]["value"]["integration"] =
                json!({"kind":"PROVIDER","method":"SHOPIFY"});
        }
        *value.pointer_mut(path).unwrap() = replacement;
        assert!(!validator::<CommandData>(false).is_valid(&value), "{path}");
        assert!(decode_command(value, key()).is_err(), "{path}");
    }
    for field in ["selection", "rateInputs", "shipping", "collection"] {
        let mut value = full();
        value["delivery"]["value"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(!validator::<CommandData>(false).is_valid(&value));
        assert!(decode_command(value, key()).is_err());
    }
    let mut value = full();
    value["delivery"]["value"]["rateInputs"]["value"]
        .as_object_mut()
        .unwrap()
        .remove("classes");
    assert!(!validator::<CommandData>(false).is_valid(&value));
    assert!(decode_command(value, key()).is_err());
    for text in [
        r#"{"expectedVersion":1,"expectedVersion":2,"delivery":{"state":"UNCHANGED"}}"#,
        r#"{"expectedVersion":1,"delivery":{"state":"UNCHANGED"},"delivery":{"state":"CLEAR"}}"#,
    ] {
        assert!(decode_command_text(text, key()).is_err());
    }
}
#[test]
fn should_preserve_null_current_view_positive_versions_and_changed_or_unchanged_outcomes() {
    let id = ProductListingId::new();
    let view = serde_json::to_value(
        DeliveryData::try_from(ProductListingDeliveryView {
            product_listing_id: id,
            version: ProductListingStorageVersion::INITIAL,
            delivery: None,
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        view,
        json!({"productListingId":id,"expectedVersion":1,"delivery":null})
    );
    assert!(validator::<DeliveryData>(true).is_valid(&view));
    let mut omitted = view;
    omitted.as_object_mut().unwrap().remove("delivery");
    assert!(!validator::<DeliveryData>(true).is_valid(&omitted));
    for outcome in [
        domain_primitives::change_outcome::ChangeOutcome::Changed,
        domain_primitives::change_outcome::ChangeOutcome::Unchanged,
    ] {
        let result = serde_json::to_value(MutationData::from(ChangeProductListingDeliveryResult {
            product_listing_id: id,
            version: ProductListingStorageVersion::INITIAL,
            outcome,
        }))
        .unwrap();
        assert!(validator::<MutationData>(true).is_valid(&result));
        assert_eq!(
            result["outcome"],
            if outcome.changed() {
                "CHANGED"
            } else {
                "UNCHANGED"
            }
        );
    }
}
