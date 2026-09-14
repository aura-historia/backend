use application::patch_field::PatchField;
use auction_core::{AuctionId, AuctionTime};
use product_listing_core::product_listing::{
    AuctionMembership, CataloguePosition, InvalidLotAuctionTiming, LotAuctionTiming, LotNumber,
    ProductListingAuction,
};
use time::OffsetDateTime;

/// Service-owned nested patch for one asserted ProductListing Auction context.
///
/// The outer `PatchField` belongs to the write command. A set outer value asserts
/// participation; its leaves preserve their own presence so ordinary writes do not
/// accidentally replace unrelated lot facts.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProductListingAuctionPatch {
    pub auction_id: PatchField<AuctionId>,
    pub lot_number: PatchField<LotNumber>,
    pub catalogue_position: PatchField<CataloguePosition>,
    pub bidding_opens: PatchField<AuctionTime>,
    pub scheduled_closes: PatchField<AuctionTime>,
    pub reported_closed_at: PatchField<OffsetDateTime>,
}

#[derive(Debug, thiserror::Error)]
pub enum ComposeProductListingAuctionPatchError {
    #[error("auction timing is invalid")]
    Timing(#[source] InvalidLotAuctionTiming),
}

/// Validates the final listing-owned Auction facts before an ordinary typed write.
pub fn validate_product_listing_auction_patch(
    existing: Option<&ProductListingAuction>,
    patch: &ProductListingAuctionPatch,
) -> Result<(), ComposeProductListingAuctionPatchError> {
    compose_product_listing_auction_patch(existing, patch).map(|_| ())
}

pub fn compose_product_listing_auction_patch(
    existing: Option<&ProductListingAuction>,
    patch: &ProductListingAuctionPatch,
) -> Result<ProductListingAuction, ComposeProductListingAuctionPatchError> {
    let existing_timing = existing.and_then(ProductListingAuction::timing);
    let timing = LotAuctionTiming::new(
        apply_option_patch(
            existing_timing
                .and_then(LotAuctionTiming::bidding_opens)
                .cloned(),
            patch.bidding_opens.clone(),
        ),
        apply_option_patch(
            existing_timing
                .and_then(LotAuctionTiming::scheduled_closes)
                .cloned(),
            patch.scheduled_closes.clone(),
        ),
        apply_option_patch(
            existing_timing.and_then(LotAuctionTiming::reported_closed_at),
            patch.reported_closed_at.clone(),
        ),
    )
    .map_err(ComposeProductListingAuctionPatchError::Timing)?;

    let has_timing = timing.bidding_opens().is_some()
        || timing.scheduled_closes().is_some()
        || timing.reported_closed_at().is_some();
    Ok(ProductListingAuction::new(
        apply_option_patch(
            existing
                .and_then(ProductListingAuction::membership)
                .map(AuctionMembership::auction_id),
            patch.auction_id.clone(),
        )
        .map(AuctionMembership::new),
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
        has_timing.then_some(timing),
    ))
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
    use product_listing_core::product_listing_auction::LotNumber;
    use time::macros::datetime;

    #[test]
    fn should_compose_present_leaves_then_validate_final_timing() {
        let existing = ProductListingAuction::new(
            None,
            Some(LotNumber::try_from("old").unwrap_or_else(|error| panic!("lot: {error}"))),
            None,
            Some(
                LotAuctionTiming::new(
                    Some(AuctionTime::instant(datetime!(2026-01-01 10:00 UTC), None)),
                    Some(AuctionTime::instant(datetime!(2026-01-01 12:00 UTC), None)),
                    None,
                )
                .unwrap_or_else(|error| panic!("timing: {error}")),
            ),
        );
        let patch = ProductListingAuctionPatch {
            lot_number: PatchField::Set(
                LotNumber::try_from("new").unwrap_or_else(|error| panic!("lot: {error}")),
            ),
            scheduled_closes: PatchField::Set(AuctionTime::instant(
                datetime!(2026-01-01 13:00 UTC),
                None,
            )),
            ..Default::default()
        };

        let result = compose_product_listing_auction_patch(Some(&existing), &patch)
            .unwrap_or_else(|error| panic!("composition: {error}"));

        assert_eq!(Some("new"), result.lot_number().map(LotNumber::as_str));
        assert_eq!(
            Some(datetime!(2026-01-01 10:00 UTC)),
            result
                .timing()
                .and_then(LotAuctionTiming::bidding_opens)
                .and_then(AuctionTime::exact_instant)
        );
        assert_eq!(
            Some(datetime!(2026-01-01 13:00 UTC)),
            result
                .timing()
                .and_then(LotAuctionTiming::scheduled_closes)
                .and_then(AuctionTime::exact_instant)
        );
    }

    #[test]
    fn should_clear_only_asserted_auction_leaves() {
        let existing = ProductListingAuction::new(
            None,
            Some(LotNumber::try_from("42").unwrap_or_else(|error| panic!("lot: {error}"))),
            Some(
                CataloguePosition::try_from(7_u64)
                    .unwrap_or_else(|error| panic!("position: {error}")),
            ),
            Some(
                LotAuctionTiming::new(
                    Some(AuctionTime::instant(datetime!(2026-01-01 10:00 UTC), None)),
                    Some(AuctionTime::instant(datetime!(2026-01-01 12:00 UTC), None)),
                    Some(datetime!(2026-01-01 12:10 UTC)),
                )
                .unwrap_or_else(|error| panic!("timing: {error}")),
            ),
        );
        let patch = ProductListingAuctionPatch {
            lot_number: PatchField::Clear,
            catalogue_position: PatchField::Clear,
            bidding_opens: PatchField::Clear,
            reported_closed_at: PatchField::Clear,
            ..Default::default()
        };

        let result = compose_product_listing_auction_patch(Some(&existing), &patch)
            .unwrap_or_else(|error| panic!("composition: {error}"));

        assert!(result.lot_number().is_none());
        assert!(result.catalogue_position().is_none());
        let timing = result
            .timing()
            .unwrap_or_else(|| panic!("scheduled close should remain"));
        assert!(timing.bidding_opens().is_none());
        assert_eq!(
            Some(datetime!(2026-01-01 12:00 UTC)),
            timing
                .scheduled_closes()
                .and_then(AuctionTime::exact_instant)
        );
        assert!(timing.reported_closed_at().is_none());
    }

    #[test]
    fn should_reject_invalid_final_timing_after_leaf_composition() {
        let existing = ProductListingAuction::new(
            None,
            None,
            None,
            Some(
                LotAuctionTiming::new(
                    Some(AuctionTime::instant(datetime!(2026-01-01 10:00 UTC), None)),
                    None,
                    None,
                )
                .unwrap_or_else(|error| panic!("timing: {error}")),
            ),
        );
        let patch = ProductListingAuctionPatch {
            scheduled_closes: PatchField::Set(AuctionTime::instant(
                datetime!(2026-01-01 09:00 UTC),
                None,
            )),
            ..Default::default()
        };

        assert!(matches!(
            validate_product_listing_auction_patch(Some(&existing), &patch),
            Err(ComposeProductListingAuctionPatchError::Timing(_))
        ));
        assert!(matches!(
            compose_product_listing_auction_patch(Some(&existing), &patch),
            Err(ComposeProductListingAuctionPatchError::Timing(_))
        ));
    }
}
