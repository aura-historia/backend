use crate::error::{ApiError, ApiErrorCode, BAD_BODY_VALUE, INVALID_OBJECT_ID};
use crate::wire::parse_path_object_id;
pub(super) use aura_historia_contracts::partner_product_listings::{
    CreateProductListingData, MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE, UpdateProductListingData,
    UpsertProductListingData, WithdrawProductListingData, prepare_batch,
};
use aura_historia_contracts::product_listing_management::MutationInputError;
use listing_source_core::ListingSourceId;
use serde::{Serialize, de::DeserializeOwned};

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct PartnerProductFailureData {
    listing_source_id: ListingSourceId,
    source_listing_id: String,
    error: ApiErrorCode,
}

pub(super) fn parse_listing_source_id(value: &str) -> Result<ListingSourceId, ApiError> {
    parse_path_object_id(value, "listingSourceId", "ListingSource")
}

pub(super) fn parse_partner_product_batch<T: DeserializeOwned>(
    body: &str,
) -> Result<Vec<T>, ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail("Body cannot be empty."));
    }

    let products: Vec<T> = serde_json::from_str(body)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))?;
    aura_historia_contracts::partner_product_listings::validate_batch_size(products.len())
        .map_err(mutation_input_error)?;

    Ok(products)
}

impl PartnerProductFailureData {
    pub(super) fn new(
        listing_source_id: ListingSourceId,
        source_listing_id: String,
        error: ApiErrorCode,
    ) -> Self {
        Self {
            listing_source_id,
            source_listing_id,
            error,
        }
    }
}

pub(super) fn mutation_input_error(error: MutationInputError) -> ApiError {
    if error.invalid_object_id {
        ApiError::bad_request(INVALID_OBJECT_ID)
            .with_path_field("auctionId")
            .with_detail(error.detail)
    } else if error.field == "title.text" {
        ApiError::bad_request(BAD_BODY_VALUE)
            .with_body_field(error.field)
            .with_detail(error.detail)
    } else {
        ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.detail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn should_preserve_canonical_path_id_validation() {
        let source = ListingSourceId::new();
        assert_eq!(
            source,
            parse_listing_source_id(&source.to_string()).unwrap()
        );
        for invalid in [
            product_listing_core::product_listing_id::ProductListingId::new().to_string(),
            source.as_uuid().to_string(),
            "ls_not-a-typeid".to_owned(),
        ] {
            assert_eq!(
                INVALID_OBJECT_ID,
                parse_listing_source_id(&invalid).unwrap_err().code()
            );
        }
    }
}
