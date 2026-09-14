use auction_core::AuctionId;
use std::fmt;
use time::OffsetDateTime;

const MAX_LOT_NUMBER_BYTES: usize = 128;

/// Source-assigned lot label.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LotNumber(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidLotNumber {
    #[error("lot number cannot be blank")]
    Blank,
    #[error("lot number cannot contain a NUL character")]
    ContainsNul,
    #[error("lot number exceeds {MAX_LOT_NUMBER_BYTES} UTF-8 bytes")]
    TooLong,
}

impl LotNumber {
    pub fn parse(value: &str) -> Result<Self, InvalidLotNumber> {
        let value = value.trim();
        if value.is_empty() {
            return Err(InvalidLotNumber::Blank);
        }
        if value.contains('\0') {
            return Err(InvalidLotNumber::ContainsNul);
        }
        if value.len() > MAX_LOT_NUMBER_BYTES {
            return Err(InvalidLotNumber::TooLong);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for LotNumber {
    type Error = InvalidLotNumber;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl TryFrom<String> for LotNumber {
    type Error = InvalidLotNumber;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl fmt::Display for LotNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<LotNumber> for String {
    fn from(value: LotNumber) -> Self {
        value.0
    }
}

/// One-based source catalogue ordering for a lot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CataloguePosition(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidCataloguePosition {
    #[error("catalogue position must be greater than zero")]
    Zero,
    #[error("catalogue position exceeds u32")]
    TooLarge,
}

impl CataloguePosition {
    pub const fn new(value: u32) -> Result<Self, InvalidCataloguePosition> {
        if value == 0 {
            return Err(InvalidCataloguePosition::Zero);
        }
        Ok(Self(value))
    }

    pub const fn value(self) -> u32 {
        self.0
    }
}

impl TryFrom<u64> for CataloguePosition {
    type Error = InvalidCataloguePosition;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        let value = u32::try_from(value).map_err(|_| InvalidCataloguePosition::TooLarge)?;
        Self::new(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidProductListingAuction {
    #[error("lot auction bidding opens after its scheduled close")]
    BiddingOpensAfterScheduledCloses,
}

/// Optional source assertions about a listing's auction context.
///
/// This value owns only listing facts. Its optional Auction ID is a reference, not
/// an Auction aggregate membership. An all-empty value normalizes to `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingAuction {
    auction_id: Option<AuctionId>,
    lot_number: Option<LotNumber>,
    catalogue_position: Option<CataloguePosition>,
    bidding_opens: Option<OffsetDateTime>,
    scheduled_closes: Option<OffsetDateTime>,
    reported_closed_at: Option<OffsetDateTime>,
}

impl ProductListingAuction {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        auction_id: Option<AuctionId>,
        lot_number: Option<LotNumber>,
        catalogue_position: Option<CataloguePosition>,
        bidding_opens: Option<OffsetDateTime>,
        scheduled_closes: Option<OffsetDateTime>,
        reported_closed_at: Option<OffsetDateTime>,
    ) -> Result<Option<Self>, InvalidProductListingAuction> {
        if bidding_opens
            .zip(scheduled_closes)
            .is_some_and(|(bidding_opens, scheduled_closes)| bidding_opens > scheduled_closes)
        {
            return Err(InvalidProductListingAuction::BiddingOpensAfterScheduledCloses);
        }

        Ok((auction_id.is_some()
            || lot_number.is_some()
            || catalogue_position.is_some()
            || bidding_opens.is_some()
            || scheduled_closes.is_some()
            || reported_closed_at.is_some())
        .then_some(Self {
            auction_id,
            lot_number,
            catalogue_position,
            bidding_opens,
            scheduled_closes,
            reported_closed_at,
        }))
    }

    pub fn normalize(auction: Option<Self>) -> Option<Self> {
        auction.filter(|auction| !auction.is_empty())
    }

    pub const fn auction_id(&self) -> Option<AuctionId> {
        self.auction_id
    }

    pub fn lot_number(&self) -> Option<&LotNumber> {
        self.lot_number.as_ref()
    }

    pub const fn catalogue_position(&self) -> Option<CataloguePosition> {
        self.catalogue_position
    }

    pub const fn bidding_opens(&self) -> Option<OffsetDateTime> {
        self.bidding_opens
    }

    pub const fn scheduled_closes(&self) -> Option<OffsetDateTime> {
        self.scheduled_closes
    }

    pub const fn reported_closed_at(&self) -> Option<OffsetDateTime> {
        self.reported_closed_at
    }

    const fn is_empty(&self) -> bool {
        self.auction_id.is_none()
            && self.lot_number.is_none()
            && self.catalogue_position.is_none()
            && self.bidding_opens.is_none()
            && self.scheduled_closes.is_none()
            && self.reported_closed_at.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn should_validate_lot_number_and_catalogue_position() {
        assert_eq!(
            Err(InvalidLotNumber::Blank),
            LotNumber::try_from(" \u{2003} ")
        );
        assert_eq!(
            Err(InvalidLotNumber::ContainsNul),
            LotNumber::try_from("12\0A")
        );
        assert_eq!(
            Err(InvalidCataloguePosition::Zero),
            CataloguePosition::new(0)
        );
        assert_eq!(
            Err(InvalidCataloguePosition::TooLarge),
            CataloguePosition::try_from(u64::from(u32::MAX) + 1)
        );
    }

    #[test]
    fn should_normalize_all_empty_facts_to_no_auction_context() {
        assert_eq!(
            Ok(None),
            ProductListingAuction::new(None, None, None, None, None, None)
        );
    }

    #[test]
    fn should_reject_bidding_open_after_scheduled_close() {
        assert_eq!(
            Err(InvalidProductListingAuction::BiddingOpensAfterScheduledCloses),
            ProductListingAuction::new(
                None,
                None,
                None,
                Some(datetime!(2026-05-14 10:00 UTC)),
                Some(datetime!(2026-05-13 10:00 UTC)),
                None,
            )
        );
    }

    #[test]
    fn should_preserve_direct_lot_timestamps() {
        let auction = ProductListingAuction::new(
            None,
            None,
            None,
            Some(datetime!(2026-05-13 10:00 UTC)),
            Some(datetime!(2026-05-14 10:00 UTC)),
            Some(datetime!(2026-05-14 10:30 UTC)),
        )
        .unwrap_or_else(|error| panic!("auction: {error}"))
        .unwrap_or_else(|| panic!("auction facts should be present"));

        assert_eq!(
            Some(datetime!(2026-05-13 10:00 UTC)),
            auction.bidding_opens()
        );
        assert_eq!(
            Some(datetime!(2026-05-14 10:00 UTC)),
            auction.scheduled_closes()
        );
        assert_eq!(
            Some(datetime!(2026-05-14 10:30 UTC)),
            auction.reported_closed_at()
        );
    }
}
