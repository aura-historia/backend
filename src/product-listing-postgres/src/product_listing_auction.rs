use auction_core::{AuctionFormat, AuctionId, AuctionName, AuctionReportedStatus, AuctionSchedule};

use localization::{Language, Localized};
use product_listing_core::product_listing_auction::{
    CataloguePosition, LotNumber, ProductListingAuction,
};
use product_listing_service::ports::{ProductListingAuctionSummary, ProductListingLot};
use std::str::FromStr;
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
#[error("persisted product-listing auction is invalid")]
pub(crate) struct ProductListingAuctionMappingError;

#[derive(Debug, Clone, Default)]
pub(crate) struct JoinedAuctionParts {
    pub(crate) auction_id: Option<uuid::Uuid>,
    pub(crate) listing_source_id: Option<uuid::Uuid>,
    pub(crate) name_text: Option<String>,
    pub(crate) name_language: Option<String>,
    pub(crate) format: Option<String>,
    pub(crate) bidding_opens_at: Option<OffsetDateTime>,
    pub(crate) live_starts_at: Option<OffsetDateTime>,
    pub(crate) lots_begin_closing_at: Option<OffsetDateTime>,
    pub(crate) scheduled_end_at: Option<OffsetDateTime>,
    pub(crate) reported_status: Option<String>,
}

pub(crate) fn map_joined_auction(
    listing: Option<ProductListingAuction>,
    listing_source_id: uuid::Uuid,
    parent: JoinedAuctionParts,
) -> Result<
    (
        Option<ProductListingAuctionSummary>,
        Option<ProductListingLot>,
    ),
    ProductListingAuctionMappingError,
> {
    let listing_auction_id = listing.as_ref().and_then(ProductListingAuction::auction_id);
    let lot = listing.as_ref().and_then(|value| {
        let lot = ProductListingLot {
            lot_number: value.lot_number().cloned(),
            catalogue_position: value.catalogue_position(),
            bidding_opens: value.bidding_opens(),
            scheduled_closes: value.scheduled_closes(),
            reported_closed_at: value.reported_closed_at(),
        };
        (lot.lot_number.is_some()
            || lot.catalogue_position.is_some()
            || lot.bidding_opens.is_some()
            || lot.scheduled_closes.is_some()
            || lot.reported_closed_at.is_some())
        .then_some(lot)
    });

    let parent_has_metadata = parent.name_text.is_some()
        || parent.name_language.is_some()
        || parent.format.is_some()
        || parent.bidding_opens_at.is_some()
        || parent.live_starts_at.is_some()
        || parent.lots_begin_closing_at.is_some()
        || parent.scheduled_end_at.is_some()
        || parent.reported_status.is_some();
    if parent.auction_id.is_none() && (parent.listing_source_id.is_some() || parent_has_metadata) {
        return Err(ProductListingAuctionMappingError);
    }
    if parent.auction_id.is_some() != parent.listing_source_id.is_some() {
        return Err(ProductListingAuctionMappingError);
    }

    let parent_auction_id = parent
        .auction_id
        .map(AuctionId::try_from)
        .transpose()
        .map_err(|_| ProductListingAuctionMappingError)?;
    if listing_auction_id != parent_auction_id {
        return Err(ProductListingAuctionMappingError);
    }

    let auction = parent_auction_id
        .map(|auction_id| {
            if parent.listing_source_id != Some(listing_source_id) {
                return Err(ProductListingAuctionMappingError);
            }
            let name = match (parent.name_text, parent.name_language) {
                (None, None) => Ok(None),
                (Some(text), Some(language)) => {
                    let payload = AuctionName::try_from(text.as_str())
                        .map_err(|_| ProductListingAuctionMappingError)?;
                    if payload.as_ref() != text {
                        return Err(ProductListingAuctionMappingError);
                    }
                    let localization =
                        Language::from_code(&language).ok_or(ProductListingAuctionMappingError)?;
                    Ok(Some(Localized::new(localization, payload)))
                }
                _ => Err(ProductListingAuctionMappingError),
            }?;
            let format = parent
                .format
                .map(|value| {
                    AuctionFormat::from_str(&value).map_err(|_| ProductListingAuctionMappingError)
                })
                .transpose()?;
            let reported_status = parent
                .reported_status
                .map(|value| {
                    AuctionReportedStatus::from_str(&value)
                        .map_err(|_| ProductListingAuctionMappingError)
                })
                .transpose()?;
            let schedule = AuctionSchedule::new(
                parent.bidding_opens_at,
                parent.live_starts_at,
                parent.lots_begin_closing_at,
                parent.scheduled_end_at,
            )
            .map_err(|_| ProductListingAuctionMappingError)?;
            Ok(ProductListingAuctionSummary {
                auction_id,
                name,
                format,
                reported_status,
                schedule,
            })
        })
        .transpose()?;

    Ok((auction, lot))
}

#[derive(Debug, Clone)]
pub(crate) struct ProductListingAuctionParts {
    pub(crate) auction_id: Option<uuid::Uuid>,
    pub(crate) lot_number: Option<String>,
    pub(crate) catalogue_position: Option<i64>,
    pub(crate) lot_bidding_opens_at: Option<OffsetDateTime>,
    pub(crate) lot_scheduled_closes_at: Option<OffsetDateTime>,
    pub(crate) lot_reported_closed_at: Option<OffsetDateTime>,
}

pub(crate) fn auction_from_parts(
    parts: ProductListingAuctionParts,
) -> Result<Option<ProductListingAuction>, ProductListingAuctionMappingError> {
    let auction_id = parts
        .auction_id
        .map(AuctionId::try_from)
        .transpose()
        .map_err(|_| ProductListingAuctionMappingError)?;
    let lot_number = parts
        .lot_number
        .map(|value| {
            let parsed = LotNumber::try_from(value.as_str())
                .map_err(|_| ProductListingAuctionMappingError)?;
            if parsed.as_str() != value {
                return Err(ProductListingAuctionMappingError);
            }
            Ok(parsed)
        })
        .transpose()?;
    let catalogue_position = parts
        .catalogue_position
        .map(|value| {
            let value = u64::try_from(value).map_err(|_| ProductListingAuctionMappingError)?;
            CataloguePosition::try_from(value).map_err(|_| ProductListingAuctionMappingError)
        })
        .transpose()?;

    ProductListingAuction::new(
        auction_id,
        lot_number,
        catalogue_position,
        parts.lot_bidding_opens_at,
        parts.lot_scheduled_closes_at,
        parts.lot_reported_closed_at,
    )
    .map_err(|_| ProductListingAuctionMappingError)
}

pub(crate) struct ProductListingAuctionWriteParts {
    pub(crate) auction_id: Option<uuid::Uuid>,
    pub(crate) lot_number: Option<String>,
    pub(crate) catalogue_position: Option<i64>,
    pub(crate) lot_bidding_opens_at: Option<OffsetDateTime>,
    pub(crate) lot_scheduled_closes_at: Option<OffsetDateTime>,
    pub(crate) lot_reported_closed_at: Option<OffsetDateTime>,
}

pub(crate) fn auction_write_parts(
    value: Option<&ProductListingAuction>,
) -> ProductListingAuctionWriteParts {
    ProductListingAuctionWriteParts {
        auction_id: value
            .and_then(ProductListingAuction::auction_id)
            .map(|value| *value.as_uuid()),
        lot_number: value
            .and_then(ProductListingAuction::lot_number)
            .map(|value| value.as_str().to_owned()),
        catalogue_position: value
            .and_then(ProductListingAuction::catalogue_position)
            .map(|value| i64::from(value.value())),
        lot_bidding_opens_at: value.and_then(ProductListingAuction::bidding_opens),
        lot_scheduled_closes_at: value.and_then(ProductListingAuction::scheduled_closes),
        lot_reported_closed_at: value.and_then(ProductListingAuction::reported_closed_at),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auction_core::{AuctionFormat, AuctionReportedStatus};
    use product_listing_core::product_listing_auction::{CataloguePosition, LotNumber};
    use time::macros::datetime;

    #[test]
    fn should_map_parent_and_independent_lot_facts() -> Result<(), Box<dyn std::error::Error>> {
        let auction_id = AuctionId::new();
        let listing_source_id = uuid::Uuid::new_v4();
        let listing = ProductListingAuction::new(
            Some(auction_id),
            Some(LotNumber::try_from("42")?),
            Some(CataloguePosition::new(7)?),
            Some(datetime!(2026-10-18 15:00 UTC)),
            Some(datetime!(2026-10-18 16:00 UTC)),
            None,
        )?
        .ok_or("listing facts should be present")?;
        let joined = JoinedAuctionParts {
            listing_source_id: Some(listing_source_id),
            auction_id: Some(*auction_id.as_uuid()),
            name_text: Some("Autumn Sale".to_owned()),
            name_language: Some("en".to_owned()),
            format: Some("TIMED".to_owned()),
            bidding_opens_at: Some(datetime!(2026-10-18 16:00 UTC)),
            live_starts_at: None,
            lots_begin_closing_at: Some(datetime!(2026-10-18 18:00 UTC)),
            scheduled_end_at: Some(datetime!(2026-10-18 20:00 UTC)),
            reported_status: Some("SCHEDULED".to_owned()),
        };

        let (auction, lot) = map_joined_auction(Some(listing), listing_source_id, joined)?;

        let auction = auction.ok_or("parent Auction should be present")?;
        assert_eq!(auction.auction_id, auction_id);
        assert_eq!(Some(AuctionFormat::Timed), auction.format);
        assert_eq!(
            Some(AuctionReportedStatus::Scheduled),
            auction.reported_status
        );
        assert_eq!(
            Some(datetime!(2026-10-18 18:00 UTC)),
            auction.schedule.lots_begin_closing()
        );
        let lot = lot.ok_or("lot should be present")?;
        assert_eq!(Some("42"), lot.lot_number.as_ref().map(LotNumber::as_str));
        assert_eq!(
            Some(7),
            lot.catalogue_position.map(CataloguePosition::value)
        );
        assert_eq!(Some(datetime!(2026-10-18 15:00 UTC)), lot.bidding_opens);
        Ok(())
    }

    #[test]
    fn should_map_standalone_lot_without_parent() -> Result<(), Box<dyn std::error::Error>> {
        let listing = ProductListingAuction::new(
            None,
            Some(LotNumber::try_from("standalone")?),
            None,
            None,
            Some(datetime!(2026-10-18 16:00 UTC)),
            None,
        )?;
        let (auction, lot) = map_joined_auction(listing, uuid::Uuid::new_v4(), Default::default())?;

        assert!(auction.is_none());
        assert!(lot.is_some());
        Ok(())
    }

    #[test]
    fn should_map_parent_without_lot_and_null_optional_metadata()
    -> Result<(), Box<dyn std::error::Error>> {
        let auction_id = AuctionId::new();
        let listing_source_id = uuid::Uuid::new_v4();
        let listing = ProductListingAuction::new(Some(auction_id), None, None, None, None, None)?;
        let (auction, lot) = map_joined_auction(
            listing,
            listing_source_id,
            JoinedAuctionParts {
                auction_id: Some(*auction_id.as_uuid()),
                listing_source_id: Some(listing_source_id),
                ..Default::default()
            },
        )?;

        assert_eq!(Some(auction_id), auction.map(|value| value.auction_id));
        assert!(lot.is_none());
        Ok(())
    }

    #[test]
    fn should_reject_a_listing_reference_without_a_joined_parent()
    -> Result<(), Box<dyn std::error::Error>> {
        let auction_id = AuctionId::new();
        let listing = ProductListingAuction::new(Some(auction_id), None, None, None, None, None)?;

        assert!(map_joined_auction(listing, uuid::Uuid::new_v4(), Default::default()).is_err());
        Ok(())
    }
}
