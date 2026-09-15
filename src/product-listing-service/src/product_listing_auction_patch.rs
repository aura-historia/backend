use application::patch_field::PatchField;
use auction_core::AuctionId;
use product_listing_core::product_listing::{
    CataloguePosition, InvalidProductListingAuction, LotNumber, ProductListingAuction,
};
use time::OffsetDateTime;

/// Service-owned leaf patches for listing-owned auction facts.
///
/// The outer `PatchField` belongs to the write command. A set outer value applies
/// only supplied leaves; no separate context-presence assertion is persisted.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProductListingAuctionPatch {
    pub auction_id: PatchField<AuctionId>,
    pub lot_number: PatchField<LotNumber>,
    pub catalogue_position: PatchField<CataloguePosition>,
    pub bidding_opens: PatchField<OffsetDateTime>,
    pub scheduled_closes: PatchField<OffsetDateTime>,
    pub reported_closed_at: PatchField<OffsetDateTime>,
}

#[derive(Debug, thiserror::Error)]
pub enum ComposeProductListingAuctionPatchError {
    #[error("auction timing is invalid")]
    Timing(#[source] InvalidProductListingAuction),
}

pub fn validate_product_listing_auction_patch(
    existing: Option<&ProductListingAuction>,
    patch: &ProductListingAuctionPatch,
) -> Result<(), ComposeProductListingAuctionPatchError> {
    compose_product_listing_auction_patch(existing, patch).map(|_| ())
}

pub fn compose_product_listing_auction_patch(
    existing: Option<&ProductListingAuction>,
    patch: &ProductListingAuctionPatch,
) -> Result<Option<ProductListingAuction>, ComposeProductListingAuctionPatchError> {
    ProductListingAuction::new(
        apply_option_patch(
            existing.and_then(ProductListingAuction::auction_id),
            patch.auction_id.clone(),
        ),
        apply_option_patch(
            existing
                .and_then(ProductListingAuction::lot_number)
                .cloned(),
            patch.lot_number.clone(),
        ),
        apply_option_patch(
            existing.and_then(ProductListingAuction::catalogue_position),
            patch.catalogue_position.clone(),
        ),
        apply_option_patch(
            existing.and_then(ProductListingAuction::bidding_opens),
            patch.bidding_opens.clone(),
        ),
        apply_option_patch(
            existing.and_then(ProductListingAuction::scheduled_closes),
            patch.scheduled_closes.clone(),
        ),
        apply_option_patch(
            existing.and_then(ProductListingAuction::reported_closed_at),
            patch.reported_closed_at.clone(),
        ),
    )
    .map_err(ComposeProductListingAuctionPatchError::Timing)
}

fn apply_option_patch<T>(current: Option<T>, patch: PatchField<T>) -> Option<T> {
    match patch {
        PatchField::Unchanged => current,
        PatchField::Set(value) => Some(value),
        PatchField::Clear => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn should_compose_direct_lot_timestamps() {
        let patch = ProductListingAuctionPatch {
            bidding_opens: PatchField::Set(datetime!(2026-01-01 10:00 UTC)),
            scheduled_closes: PatchField::Set(datetime!(2026-01-01 12:00 UTC)),
            ..Default::default()
        };

        let auction = compose_product_listing_auction_patch(None, &patch)
            .unwrap_or_else(|error| panic!("composition: {error}"))
            .unwrap_or_else(|| panic!("lot facts should be present"));

        assert_eq!(
            Some(datetime!(2026-01-01 10:00 UTC)),
            auction.bidding_opens()
        );
        assert_eq!(
            Some(datetime!(2026-01-01 12:00 UTC)),
            auction.scheduled_closes()
        );
    }

    #[test]
    fn should_normalize_all_cleared_facts_to_none() {
        let existing = ProductListingAuction::new(
            None,
            None,
            None,
            Some(datetime!(2026-01-01 10:00 UTC)),
            None,
            None,
        )
        .unwrap_or_else(|error| panic!("auction: {error}"));
        let patch = ProductListingAuctionPatch {
            bidding_opens: PatchField::Clear,
            ..Default::default()
        };

        assert_eq!(
            None,
            compose_product_listing_auction_patch(existing.as_ref(), &patch)
                .unwrap_or_else(|error| panic!("composition: {error}"))
        );
    }

    #[test]
    fn should_reject_open_after_close() {
        let patch = ProductListingAuctionPatch {
            bidding_opens: PatchField::Set(datetime!(2026-01-01 12:00 UTC)),
            scheduled_closes: PatchField::Set(datetime!(2026-01-01 10:00 UTC)),
            ..Default::default()
        };

        assert!(matches!(
            validate_product_listing_auction_patch(None, &patch),
            Err(ComposeProductListingAuctionPatchError::Timing(_))
        ));
    }
}
