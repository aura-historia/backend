//! Native UUID reference codec for persisted shipping terms.
use application::error::{BoxError, box_error};
use listing_source_core::ListingSourceId;
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};
use shipping_core::terms::DeliveryTerms;
use shipping_json::ShippingTermsReferenceCodec;
use uuid::Uuid;
pub(super) struct StoredReferences;
impl shipping_json::listing_delivery::DeliveryReferenceCodec for StoredReferences {
    fn decode_source(value: &str) -> Result<ListingSourceId, BoxError> {
        ListingSourceId::try_from(native_id(value)?).map_err(box_error)
    }
    fn encode_source(value: ListingSourceId) -> String {
        value.into_uuid().to_string()
    }
    fn decode_profile(value: &str) -> Result<shipping_core::profile::ShippingProfileId, BoxError> {
        shipping_core::profile::ShippingProfileId::try_from(native_id(value)?).map_err(box_error)
    }
    fn encode_profile(value: shipping_core::profile::ShippingProfileId) -> String {
        value.into_uuid().to_string()
    }
}
fn native_id(value: &str) -> Result<Uuid, BoxError> {
    let id: Uuid = value.parse()?;
    if id.to_string() != value {
        return Err(box_error(std::io::Error::other(
            "noncanonical shipping storage reference",
        )));
    }
    Ok(id)
}
impl ShippingTermsReferenceCodec for StoredReferences {
    fn decode_party(value: &str) -> Result<PartyId, BoxError> {
        PartyId::try_from(native_id(value)?).map_err(box_error)
    }
    fn encode_party(value: PartyId) -> String {
        value.into_uuid().to_string()
    }
    fn decode_location(value: &str) -> Result<PartyLocationId, BoxError> {
        PartyLocationId::try_from(native_id(value)?).map_err(box_error)
    }
    fn encode_location(value: PartyLocationId) -> String {
        value.into_uuid().to_string()
    }
}
pub(super) fn decode(
    value: serde_json::Value,
    source: ListingSourceId,
) -> Result<DeliveryTerms, BoxError> {
    shipping_json::decode::<StoredReferences>(value, source)
}
pub(super) fn encode(value: &DeliveryTerms) -> Result<serde_json::Value, BoxError> {
    shipping_json::encode::<StoredReferences>(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shipping_core::fixtures::gallery_terms;
    #[test]
    fn gallery_roundtrip_rejects_every_missing_field_and_unsupported_code() {
        let source = ListingSourceId::new();
        let terms = gallery_terms(8000);
        let value = encode(&terms).unwrap();
        assert_eq!(terms, decode(value.clone(), source).unwrap());
        fn visit(
            root: &serde_json::Value,
            current: &serde_json::Value,
            path: &str,
            source: ListingSourceId,
        ) {
            match current {
                serde_json::Value::Object(fields) => {
                    for (key, value) in fields {
                        let mut missing = root.clone();
                        missing
                            .pointer_mut(path)
                            .unwrap()
                            .as_object_mut()
                            .unwrap()
                            .remove(key);
                        assert!(
                            decode(missing, source).is_err(),
                            "missing required {path}/{key}"
                        );
                        visit(root, value, &format!("{path}/{key}"), source);
                    }
                }
                serde_json::Value::Array(values) => {
                    for (i, value) in values.iter().enumerate() {
                        visit(root, value, &format!("{path}/{i}"), source)
                    }
                }
                _ => {}
            }
        }
        // Components are a dictionary, not a fixed schema, and the gallery has no entries.
        visit(&value, &value, "", source);
        for (path, replacement) in [
            ("/schema", serde_json::json!("SHIP_TERMS_FUTURE")),
            (
                "/shipping/coverage/completeness",
                serde_json::json!("UNKNOWN"),
            ),
            (
                "/shipping/coverage/zones/0/services/0/eligibility",
                serde_json::json!("MAYBE"),
            ),
            (
                "/shipping/coverage/zones/0/services/0/rate/price/amount/currency",
                serde_json::json!("eur"),
            ),
            (
                "/shipping/coverage/zones/0/services/0/rate/basis",
                serde_json::json!("UNKNOWN"),
            ),
        ] {
            let mut corrupt = value.clone();
            *corrupt.pointer_mut(path).unwrap() = replacement;
            assert!(decode(corrupt, source).is_err());
        }
    }
    #[test]
    fn tagged_empty_variants_reject_unexpected_fields() {
        for value in [
            serde_json::json!({"schema":"SHIP_TERMS_V1","shipping":{"kind":"PROHIBITED","extra":true},"collection":null}),
            serde_json::json!({"schema":"SHIP_TERMS_V1","shipping":null,"collection":{"applicability":{"class":null,"variants":null,"quantity":null,"threshold":null},"dependency":"INDEPENDENT","eligibility":null,"site":null,"arrangement":{"kind":"BY_APPOINTMENT","extra":true}}}),
        ] {
            assert!(decode(value, ListingSourceId::new()).is_err());
        }
    }
    #[test]
    fn numeric_ranges_geography_and_storage_references_use_validated_constructors() {
        let source = ListingSourceId::new();
        let base = encode(&gallery_terms(8000)).unwrap();
        let mut inverted = base.clone();
        inverted["shipping"]["coverage"]["zones"][0]["services"][0]["rate"]["price"] = serde_json::json!({"kind":"ESTIMATED_RANGE","minimum":{"amount_minor":9000,"currency":"EUR"},"maximum":{"amount_minor":8000,"currency":"EUR"}});
        assert!(decode(inverted, source).is_err());
        let mut mixed = base.clone();
        mixed["shipping"]["coverage"]["zones"][0]["services"][0]["rate"]["price"] = serde_json::json!({"kind":"ESTIMATED_RANGE","minimum":{"amount_minor":8000,"currency":"EUR"},"maximum":{"amount_minor":9000,"currency":"USD"}});
        assert!(decode(mixed, source).is_err());
        let id = PartyLocationId::new();
        let mut reference = base.clone();
        reference["collection"]["site"] = serde_json::json!({"kind":"PARTY_LOCATION","party_location_id":id.into_uuid().to_string()});
        assert!(decode(reference.clone(), source).is_ok());
        reference["collection"]["site"]["party_location_id"] = serde_json::json!(id.to_string());
        assert!(decode(reference, source).is_err());
        for site in [
            serde_json::json!({"kind":"ASSERTED","geography":{"reference_release":"iso-codes-4.20.1","address_text":null,"country":"XX","subdivision":null,"postal_code":null},"position":null}),
            serde_json::json!({"kind":"ASSERTED","geography":null,"position":null}),
            serde_json::json!({"kind":"ASSERTED","geography":null,"position":{"point":{"latitude_degrees":91.0,"longitude_degrees":0.0},"precision":"PREMISES","accuracy_metres":null}}),
        ] {
            let mut value = base.clone();
            value["collection"]["site"] = site;
            assert!(decode(value, source).is_err());
        }
    }
}
