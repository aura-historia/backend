use crate::{
    error::{ApiError, BAD_BODY_VALUE, INVALID_OBJECT_ID},
    patch_value::PatchValue,
};
use application::patch_field::PatchField;
use aura_historia_contracts::product_listing_fields;
pub(crate) use aura_historia_contracts::product_listing_fields::{
    ProductListingAuctionData, product_images,
};
use product_listing_service::product_listing_auction_patch::ProductListingAuctionPatch;
pub(crate) fn auction_patch(
    value: PatchValue<ProductListingAuctionData>,
) -> Result<PatchField<ProductListingAuctionPatch>, ApiError> {
    product_listing_fields::auction_patch(value).map_err(|error| {
        if error.invalid_object_id {
            ApiError::bad_request(INVALID_OBJECT_ID)
                .with_path_field("auctionId")
                .with_detail(error.detail)
        } else {
            ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.detail)
        }
    })
}
