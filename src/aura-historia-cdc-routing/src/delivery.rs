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

pub(super) fn validate(
    value: &serde_json::Value,
    changed: bool,
) -> Result<(), super::CdcRouteError> {
    let bad = || super::CdcRouteError::InvalidProductListingEventPayload;
    if changed {
        let object = value.as_object().ok_or_else(bad)?;
        if object.len() != 2 || !object.contains_key("previous") || !object.contains_key("current")
        {
            return Err(bad());
        }
        let parse = |v: &serde_json::Value| {
            if v.is_null() {
                Ok(None)
            } else {
                decode(v.clone()).map(Some).map_err(|_| bad())
            }
        };
        let previous = parse(&object["previous"])?;
        let current = parse(&object["current"])?;
        if previous == current
            || previous
                .as_ref()
                .zip(current.as_ref())
                .is_some_and(|(p, c)| p.source_id() != c.source_id())
        {
            return Err(bad());
        }
    } else {
        decode(value.clone()).map_err(|_| bad())?;
    }
    Ok(())
}
