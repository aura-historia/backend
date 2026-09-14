use auction_core::AuctionId;
use product_listing_core::product_listing_auction::{
    CataloguePosition, LotNumber, ProductListingAuction,
};
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
#[error("persisted product-listing auction is invalid")]
pub(crate) struct ProductListingAuctionMappingError;

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
