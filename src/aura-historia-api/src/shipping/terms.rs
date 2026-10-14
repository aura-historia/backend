//! Public TypeID reference codec for advertised shipping terms.
use application::error::{BoxError, box_error};
use listing_source_core::ListingSourceId;
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};
use shipping_core::terms::DeliveryTerms;
use shipping_json::ShippingTermsReferenceCodec;
struct PublicReferences;
impl ShippingTermsReferenceCodec for PublicReferences {
    fn decode_party(value: &str) -> Result<PartyId, BoxError> {
        value.parse().map_err(box_error)
    }
    fn encode_party(value: PartyId) -> String {
        value.to_string()
    }
    fn decode_location(value: &str) -> Result<PartyLocationId, BoxError> {
        value.parse().map_err(box_error)
    }
    fn encode_location(value: PartyLocationId) -> String {
        value.to_string()
    }
}
pub(super) fn decode(
    value: serde_json::Value,
    source: ListingSourceId,
) -> Result<DeliveryTerms, BoxError> {
    shipping_json::decode::<PublicReferences>(value, source)
}
pub(super) fn encode(value: &DeliveryTerms) -> Result<serde_json::Value, BoxError> {
    shipping_json::encode::<PublicReferences>(value)
}
