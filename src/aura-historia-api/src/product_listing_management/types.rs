use crate::error::{ApiError, BAD_BODY_VALUE, INVALID_OBJECT_ID};
use aura_historia_contracts::product_listing_management::MutationInputError;
pub(crate) use aura_historia_contracts::product_listing_management::{
    DetailsData, MutationData, UpdateData, VersionData, version,
};
impl From<MutationInputError> for ApiError {
    fn from(error: MutationInputError) -> Self {
        Self::bad_request(if error.invalid_object_id {
            INVALID_OBJECT_ID
        } else {
            BAD_BODY_VALUE
        })
        .with_body_field(error.field)
        .with_detail(error.detail)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use application::patch_field::PatchField;
    use product_listing_core::product_listing_id::ProductListingId;
    use serde_json::json;

    #[test]
    fn requires_storage_representable_version_and_rejects_read_only_fields() {
        for body in [
            json!({}),
            json!({"expectedVersion":0}),
            json!({"expectedVersion":u64::MAX}),
            json!({"expectedVersion":null}),
            json!({"expectedVersion":1,"title":{"text":"🌿✨","language":"en"}}),
            json!({"expectedVersion":1,"lifecycle":"ACTIVE"}),
            json!({"expectedVersion":1,"productListingId":"x"}),
            json!({"expectedVersion":1,"saleObservation":null}),
            json!({"expectedVersion":1,"description":"x"}),
        ] {
            assert!(
                serde_json::from_value::<UpdateData>(body)
                    .and_then(|data| data
                        .into_command(ProductListingId::new())
                        .map_err(serde::de::Error::custom))
                    .is_err()
            );
        }
    }
    #[test]
    fn rejects_unknown_nested_fields_and_nonnullable_containers() {
        for body in [
            json!({"price":{"type":"ON_REQUEST","amount":1}}),
            json!({"priceEstimateMin":{"currency":"EUR","amount":1,"extra":true}}),
            json!({"priceEstimateMin":{"type":"ON_REQUEST"}}),
            json!({"price":{"type":"MONETARY","currency":"EUR","amount":u64::MAX}}),
            json!({"auction":{"other":1}}),
            json!({"auction":{"timing":{"other":1}}}),
            json!({"auction":null}),
            json!({"images":null}),
            json!({"images":[null]}),
            json!({"url":null}),
        ] {
            let mut body = body;
            body["expectedVersion"] = json!(1);
            assert!(
                serde_json::from_value::<UpdateData>(body)
                    .and_then(|data| data
                        .into_command(ProductListingId::new())
                        .map_err(serde::de::Error::custom))
                    .is_err()
            );
        }
    }
    #[test]
    fn preserves_omissions_and_clears_only_explicit_optional_facts() {
        let command = serde_json::from_value::<UpdateData>(json!({"expectedVersion":1,"price":null,"priceEstimateMin":null,"availability":null,"images":[],"auction":{"auctionId":null,"timing":null}})).unwrap().into_command(ProductListingId::new()).unwrap();
        assert!(matches!(command.patch.price, PatchField::Clear));
        assert!(matches!(
            command.patch.price_estimate_min,
            PatchField::Clear
        ));
        assert!(matches!(
            command.patch.price_estimate_max,
            PatchField::Unchanged
        ));
        assert!(matches!(command.patch.availability, PatchField::Clear));
        assert!(matches!(command.patch.url, PatchField::Unchanged));
        assert!(matches!(command.patch.images, PatchField::Set(ref values) if values.is_empty()));
        let PatchField::Set(auction) = command.patch.auction else {
            panic!("auction patch")
        };
        assert!(matches!(auction.auction_id, PatchField::Clear));
        assert!(matches!(auction.lot_number, PatchField::Unchanged));
        assert!(matches!(auction.bidding_opens, PatchField::Unchanged));
    }
}
