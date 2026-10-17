//! Public TypeID delivery command protocol shared by API and asynchronous ingestion.
use crate::codec::{CodecError, WirePatch, map_patch};
use application::{
    error::{BoxError, box_error},
    patch_field::PatchField,
};
use listing_source_core::ListingSourceId;
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};
use product_listing_core::product_listing_id::ProductListingKey;
use product_listing_service::{
    ports::ProductListingStorageVersion,
    use_cases::{ChangeProductListingDeliveryCommand, ProductListingDeliveryPatch},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use shipping_core::profile::ShippingProfileId;
use shipping_json::{
    ShippingTermsReferenceCodec,
    listing_delivery::{self, DeliveryReferenceCodec},
};
pub struct PublicDeliveryReferences;
impl ShippingTermsReferenceCodec for PublicDeliveryReferences {
    fn decode_party(v: &str) -> Result<PartyId, BoxError> {
        v.parse().map_err(box_error)
    }
    fn encode_party(v: PartyId) -> String {
        v.to_string()
    }
    fn decode_location(v: &str) -> Result<PartyLocationId, BoxError> {
        v.parse().map_err(box_error)
    }
    fn encode_location(v: PartyLocationId) -> String {
        v.to_string()
    }
}
impl DeliveryReferenceCodec for PublicDeliveryReferences {
    fn decode_source(v: &str) -> Result<ListingSourceId, BoxError> {
        v.parse().map_err(box_error)
    }
    fn encode_source(v: ListingSourceId) -> String {
        v.to_string()
    }
    fn decode_profile(v: &str) -> Result<ShippingProfileId, BoxError> {
        v.parse().map_err(box_error)
    }
    fn encode_profile(v: ShippingProfileId) -> String {
        v.to_string()
    }
}
/// Shared public and queue protocol; timestamps always use RFC3339, never receipt time.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportData {
    integration: Value,
    sequence: u64,
    acquired_at: String,
    request_key: String,
    #[serde(default)]
    object_key: Option<String>,
    #[serde(default)]
    observation: Option<ObservationData>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ObservationData {}
impl ImportData {
    pub fn into_command(
        self,
    ) -> Result<shipping_service::imports::ShippingImportWrite, CodecError> {
        use time::{OffsetDateTime, format_description::well_known::Rfc3339};
        let acquired_at =
            OffsetDateTime::parse(&self.acquired_at, &Rfc3339).map_err(|_| CodecError::Invalid)?;
        let observation = self
            .observation
            .map(|_| shipping_service::imports::ShippingObservation { acquired_at });
        let write = shipping_service::imports::ShippingImportWrite {
            integration: invalid(listing_delivery::decode_integration(self.integration))?,
            sequence: self.sequence,
            acquired_at,
            request_key: self.request_key,
            object_key: self.object_key,
            observation,
        };
        write.validate().map_err(|_| CodecError::Invalid)?;
        Ok(write)
    }
    fn from_command(
        w: &shipping_service::imports::ShippingImportWrite,
    ) -> Result<Self, CodecError> {
        let observation = w.observation.as_ref().map(|_| ObservationData {});
        Ok(Self {
            integration: invalid(listing_delivery::encode_integration(&w.integration))?,
            sequence: w.sequence,
            acquired_at: w
                .acquired_at
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(|_| CodecError::Invalid)?,
            request_key: w.request_key.clone(),
            object_key: w.object_key.clone(),
            observation,
        })
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CommandData {
    expected_version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    import: Option<ImportData>,
    delivery: WirePatch<PatchData>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PatchData {
    integration: Value,
    selection: WirePatch<Value>,
    rate_inputs: WirePatch<Value>,
    shipping: WirePatch<Value>,
    collection: WirePatch<Value>,
}
fn invalid<T>(v: Result<T, BoxError>) -> Result<T, CodecError> {
    v.map_err(|_| CodecError::Invalid)
}
pub fn decode_command(
    value: Value,
    key: ProductListingKey,
) -> Result<ChangeProductListingDeliveryCommand, CodecError> {
    let data: CommandData = serde_json::from_value(value).map_err(|_| CodecError::Invalid)?;
    decode_data(data, key)
}
/// Parse typed command fields directly, preserving duplicate-field rejection at HTTP ingress.
pub fn decode_command_text(
    body: &str,
    key: ProductListingKey,
) -> Result<ChangeProductListingDeliveryCommand, CodecError> {
    let data: CommandData = serde_json::from_str(body).map_err(|_| CodecError::Invalid)?;
    decode_data(data, key)
}
fn decode_data(
    data: CommandData,
    key: ProductListingKey,
) -> Result<ChangeProductListingDeliveryCommand, CodecError> {
    if data.expected_version > i64::MAX as u64 {
        return Err(CodecError::Invalid);
    }
    let source = key.listing_source_id;
    let delivery = map_patch(data.delivery, |p| {
        Ok(ProductListingDeliveryPatch {
            integration: invalid(listing_delivery::decode_integration(p.integration))?,
            selection: map_patch(p.selection, |v| {
                invalid(listing_delivery::decode_selection::<PublicDeliveryReferences>(v, source))
            })?,
            rate_inputs: map_patch(p.rate_inputs, |v| {
                invalid(listing_delivery::decode_inputs(v, source))
            })?,
            shipping: map_patch(p.shipping, |v| {
                invalid(listing_delivery::decode_shipping::<PublicDeliveryReferences>(v, source))
            })?,
            collection: map_patch(p.collection, |v| {
                invalid(listing_delivery::decode_collection::<
                    PublicDeliveryReferences,
                >(v, source))
            })?,
        })
    })?;
    let command = ChangeProductListingDeliveryCommand {
        import: data.import.map(ImportData::into_command).transpose()?,
        product_key: key,
        expected_version: ProductListingStorageVersion::try_from(data.expected_version)
            .map_err(|_| CodecError::Invalid)?,
        delivery,
    };
    if let PatchField::Set(p) = &command.delivery {
        p.apply(source, None).map_err(|_| CodecError::Invalid)?;
    }
    Ok(command)
}
fn patch<T>(
    p: &PatchField<T>,
    f: impl FnOnce(&T) -> Result<Value, BoxError>,
) -> Result<WirePatch<Value>, CodecError> {
    Ok(match p {
        PatchField::Unchanged => WirePatch::Unchanged,
        PatchField::Clear => WirePatch::Clear,
        PatchField::Set(v) => WirePatch::Set(invalid(f(v))?),
    })
}
pub fn encode_command(c: &ChangeProductListingDeliveryCommand) -> Result<Value, CodecError> {
    let delivery = match &c.delivery {
        PatchField::Unchanged => WirePatch::Unchanged,
        PatchField::Clear => WirePatch::Clear,
        PatchField::Set(p) => WirePatch::Set(PatchData {
            integration: invalid(listing_delivery::encode_integration(&p.integration))?,
            selection: patch(
                &p.selection,
                listing_delivery::encode_selection::<PublicDeliveryReferences>,
            )?,
            rate_inputs: patch(&p.rate_inputs, listing_delivery::encode_inputs)?,
            shipping: patch(
                &p.shipping,
                listing_delivery::encode_shipping::<PublicDeliveryReferences>,
            )?,
            collection: patch(
                &p.collection,
                listing_delivery::encode_collection::<PublicDeliveryReferences>,
            )?,
        }),
    };
    serde_json::to_value(CommandData {
        expected_version: c.expected_version.into_inner(),
        import: c
            .import
            .as_ref()
            .map(ImportData::from_command)
            .transpose()?,
        delivery,
    })
    .map_err(|_| CodecError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use product_listing_service::use_cases::{
        ProductListingIngestionActor, ProductListingIngestionCommandId,
        ProductListingIngestionIntent, ProductListingIngestionMessage,
        ProductListingIngestionMetadata, ProductListingIngestionOperation,
        ProductListingIngestionSubmissionId,
    };
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
    fn version_three_delivery_preserves_retry_and_fifo_identity() {
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
            serde_json::json!(2),
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
        wrong["schemaVersion"] = serde_json::json!(1);
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
