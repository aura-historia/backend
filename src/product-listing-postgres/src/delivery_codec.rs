use application::error::{BoxError, box_error};
use listing_source_core::ListingSourceId;
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};
use shipping_core::profile::ShippingProfileId;
use shipping_json::{ShippingTermsReferenceCodec, listing_delivery::DeliveryReferenceCodec};
pub(crate) struct StoredDeliveryReferences;
fn id(v: &str) -> Result<uuid::Uuid, BoxError> {
    let id: uuid::Uuid = v.parse()?;
    if id.to_string() != v {
        return Err(box_error(std::io::Error::other(
            "noncanonical delivery identity",
        )));
    }
    Ok(id)
}
impl ShippingTermsReferenceCodec for StoredDeliveryReferences {
    fn decode_party(v: &str) -> Result<PartyId, BoxError> {
        PartyId::try_from(id(v)?).map_err(box_error)
    }
    fn encode_party(v: PartyId) -> String {
        v.into_uuid().to_string()
    }
    fn decode_location(v: &str) -> Result<PartyLocationId, BoxError> {
        PartyLocationId::try_from(id(v)?).map_err(box_error)
    }
    fn encode_location(v: PartyLocationId) -> String {
        v.into_uuid().to_string()
    }
}
impl DeliveryReferenceCodec for StoredDeliveryReferences {
    fn decode_source(v: &str) -> Result<ListingSourceId, BoxError> {
        ListingSourceId::try_from(id(v)?).map_err(box_error)
    }
    fn encode_source(v: ListingSourceId) -> String {
        v.into_uuid().to_string()
    }
    fn decode_profile(v: &str) -> Result<ShippingProfileId, BoxError> {
        ShippingProfileId::try_from(id(v)?).map_err(box_error)
    }
    fn encode_profile(v: ShippingProfileId) -> String {
        v.into_uuid().to_string()
    }
}
pub(crate) fn decode(
    v: serde_json::Value,
) -> Result<product_listing_core::delivery::ListingDelivery, BoxError> {
    shipping_json::listing_delivery::decode::<StoredDeliveryReferences>(v)
}
pub(crate) fn encode(
    v: &product_listing_core::delivery::ListingDelivery,
) -> Result<serde_json::Value, BoxError> {
    shipping_json::listing_delivery::encode::<StoredDeliveryReferences>(v)
}
