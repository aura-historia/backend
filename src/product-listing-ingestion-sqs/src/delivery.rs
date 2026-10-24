//! Compatibility boundary for the shared public delivery command codec.
use crate::codec::CodecError;
pub use aura_historia_contracts::product_listing_delivery::PublicDeliveryReferences;
pub use aura_historia_contracts::shipping::imports::ImportData;
use product_listing_core::product_listing_id::ProductListingKey;
use product_listing_service::use_cases::ChangeProductListingDeliveryCommand;
use serde_json::Value;
pub fn decode_command(
    value: Value,
    key: ProductListingKey,
) -> Result<ChangeProductListingDeliveryCommand, CodecError> {
    aura_historia_contracts::product_listing_delivery::decode_command(value, key)
        .map_err(|_| CodecError::Invalid)
}
pub fn decode_command_text(
    body: &str,
    key: ProductListingKey,
) -> Result<ChangeProductListingDeliveryCommand, CodecError> {
    aura_historia_contracts::product_listing_delivery::decode_command_text(body, key)
        .map_err(|_| CodecError::Invalid)
}
pub fn encode_command(command: &ChangeProductListingDeliveryCommand) -> Result<Value, CodecError> {
    aura_historia_contracts::product_listing_delivery::encode_command(command)
        .map_err(|_| CodecError::Invalid)
}
#[cfg(test)]
mod tests {
    use super::*;
    use application::patch_field::PatchField;
    use listing_source_core::ListingSourceId;
    use product_listing_service::use_cases::{
        ProductListingIngestionActor, ProductListingIngestionCommandId,
        ProductListingIngestionIntent, ProductListingIngestionMessage,
        ProductListingIngestionMetadata, ProductListingIngestionOperation,
        ProductListingIngestionSubmissionId,
    };
    use product_listing_service::{
        ports::ProductListingStorageVersion, use_cases::ProductListingDeliveryPatch,
    };
    use shipping_core::profile::ShippingProfileId;
    use shipping_core::{
        selection::{ComponentOverride, ProfileSelection},
        terms::ShippingPolicy,
    };
    fn command() -> ChangeProductListingDeliveryCommand {
        ChangeProductListingDeliveryCommand {
            import: None,
            product_key: ProductListingKey::new(
                ListingSourceId::new(),
                "painting".try_into().unwrap(),
            ),
            expected_version: ProductListingStorageVersion::INITIAL,
            delivery: PatchField::Set(ProductListingDeliveryPatch {
                selection: PatchField::Set(ProfileSelection::Explicit(ShippingProfileId::new())),
                shipping: PatchField::Set(ComponentOverride::Replace(ShippingPolicy::Prohibited)),
                ..Default::default()
            }),
        }
    }
    #[test]
    fn public_command_roundtrip_is_strict_and_uses_validated_values() {
        let c = command();
        let value = encode_command(&c).unwrap();
        assert_eq!(
            c,
            decode_command(value.clone(), c.product_key.clone()).unwrap()
        );
        let mut invalid = value.clone();
        invalid["expectedVersion"] = serde_json::json!(0);
        assert!(decode_command(invalid, c.product_key.clone()).is_err());
        let mut invalid = value.clone();
        invalid["delivery"]["value"]["selection"]["value"]["profile_id"] =
            serde_json::json!(ShippingProfileId::new().into_uuid().to_string());
        assert!(decode_command(invalid, c.product_key.clone()).is_err());
        let mut invalid = value.clone();
        invalid["delivery"]["value"]["collection"] =
            serde_json::json!({"state":"RECURSIVE_MERGE","value":{}});
        assert!(decode_command(invalid, c.product_key.clone()).is_err());
        let mut invalid = value.clone();
        invalid["delivery"]["value"]
            .as_object_mut()
            .unwrap()
            .remove("rateInputs");
        assert!(decode_command(invalid, c.product_key.clone()).is_err());
        let mut invalid = value;
        invalid["extra"] = serde_json::json!(true);
        assert!(decode_command(invalid, c.product_key).is_err());
    }
    #[test]
    fn version_one_delivery_preserves_retry_and_fifo_identity() {
        let c = command();
        let intent = ProductListingIngestionIntent::ChangeDelivery(c.clone());
        let message = ProductListingIngestionMessage {
            intent,
            metadata: ProductListingIngestionMetadata {
                submission_id: ProductListingIngestionSubmissionId::from_wire(&format!(
                    "plis1_{}",
                    "1".repeat(64)
                ))
                .unwrap(),
                command_id: ProductListingIngestionCommandId::from_wire(&format!(
                    "plic1_{}",
                    "2".repeat(64)
                ))
                .unwrap(),
                index: 0,
                input_count: 1,
                listing_source_id: c.product_key.listing_source_id,
                operation: ProductListingIngestionOperation::ChangeDelivery,
                actor: ProductListingIngestionActor::System,
                request_id: "synthetic-request".into(),
                correlation_id: "synthetic-correlation".into(),
            },
        };
        let first = crate::codec::envelope(&message).unwrap();
        let body = crate::codec::encode(&message).unwrap();
        assert_eq!(
            serde_json::json!(1),
            serde_json::from_str::<Value>(&body).unwrap()["schemaVersion"]
        );
        assert_eq!(
            message.intent,
            crate::codec::decode(&body).unwrap().into_intent().unwrap()
        );
        let mut retry = message.clone();
        retry.metadata.request_id = "retry-request".into();
        let second = crate::codec::envelope(&retry).unwrap();
        assert_eq!(
            crate::codec::semantic_fingerprint(&first).unwrap(),
            crate::codec::semantic_fingerprint(&second).unwrap()
        );
        assert_eq!(
            crate::codec::fifo_deduplication_id(&first).unwrap(),
            crate::codec::fifo_deduplication_id(&second).unwrap()
        );
        assert_eq!(
            crate::codec::fifo_group_id(&first).unwrap(),
            crate::codec::fifo_group_id(&second).unwrap()
        );
        let mut wrong: Value = serde_json::from_str(&body).unwrap();
        wrong["schemaVersion"] = serde_json::json!(2);
        assert!(crate::codec::decode(&wrong.to_string()).is_err());
        wrong["schemaVersion"] = serde_json::json!(3);
        assert!(crate::codec::decode(&wrong.to_string()).is_err());
        let mut changed = message;
        let ProductListingIngestionIntent::ChangeDelivery(ref mut c) = changed.intent else {
            panic!()
        };
        c.delivery = PatchField::Clear;
        assert_ne!(
            crate::codec::semantic_fingerprint(&first).unwrap(),
            crate::codec::semantic_fingerprint(&crate::codec::envelope(&changed).unwrap()).unwrap()
        );
    }
    #[test]
    fn rate_inputs_preserve_opaque_identity_and_invalid_geography_is_rejected() {
        use shipping_core::inputs::{
            ClassMembership, FactCompleteness, ListingRateInputs, SourceShippingClass,
            SourceVariant,
        };
        let mut c = command();
        let source = c.product_key.listing_source_id;
        let inputs = ListingRateInputs::new(
            Some(
                ClassMembership::new(
                    vec![
                        SourceShippingClass::new(source, "Provider/Class:FRAGILE".into()).unwrap(),
                    ],
                    FactCompleteness::Partial,
                )
                .unwrap(),
            ),
            Some(vec![
                SourceVariant::new(source, "gid://provider/Variant/2".into()).unwrap(),
                SourceVariant::new(source, "gid://provider/Variant/3".into()).unwrap(),
            ]),
        )
        .unwrap();
        let PatchField::Set(ref mut p) = c.delivery else {
            panic!()
        };
        p.rate_inputs = PatchField::Set(inputs);
        let encoded = encode_command(&c).unwrap();
        assert_eq!(
            c,
            decode_command(encoded.clone(), c.product_key.clone()).unwrap()
        );
        let mut corrupt = encoded;
        corrupt["delivery"]["value"]["collection"] = serde_json::json!({"state":"SET","value":{"kind":"REPLACE","value":{"applicability":{"class":null,"variants":null,"quantity":null,"threshold":null},"dependency":"INDEPENDENT","eligibility":"OFFERED","site":{"kind":"ASSERTED","geography":{"reference_release":"iso-codes-4.20.1","address_text":null,"country":"XX","subdivision":null,"postal_code":null},"position":null},"arrangement":null}}});
        assert!(decode_command(corrupt, c.product_key.clone()).is_err());
        let body = encode_command(&c).unwrap().to_string();
        let duplicate = body.replacen(
            "\"expectedVersion\":1",
            "\"expectedVersion\":1,\"expectedVersion\":2",
            1,
        );
        assert!(decode_command_text(&duplicate, c.product_key).is_err());
    }
    #[test]
    fn canonical_incremental_import_fixtures_roundtrip_and_reject_invalid_metadata() {
        let fixtures: Value =
            serde_json::from_str(include_str!("../../../docs/fixtures/shipping-imports.json"))
                .unwrap();
        let key = ProductListingKey::new(
            "ls_01h455vb4pex5vy7enb1p677v1".parse().unwrap(),
            "synthetic-painting".try_into().unwrap(),
        );
        for name in ["membership", "providerMembership"] {
            let command = decode_command(fixtures[name].clone(), key.clone()).unwrap();
            assert!(command.import.is_some());
            assert_eq!(
                command,
                decode_command(encode_command(&command).unwrap(), key.clone()).unwrap()
            );
            let mut invalid = fixtures[name].clone();
            invalid["import"]["sequence"] = serde_json::json!(0);
            assert!(decode_command(invalid, key.clone()).is_err());
            let mut invalid = fixtures[name].clone();
            invalid["import"]["acquiredAt"] = serde_json::json!("not-a-timestamp");
            assert!(decode_command(invalid, key.clone()).is_err());
            let mut invalid = fixtures[name].clone();
            invalid["import"]["providerPayload"] = serde_json::json!({});
            assert!(decode_command(invalid, key.clone()).is_err());
            let mut invalid = fixtures[name].clone();
            invalid["import"]["observation"] =
                serde_json::json!({"expiresAt":"2026-10-22T10:00:00Z"});
            assert!(decode_command(invalid, key.clone()).is_err());
        }
    }
}
