use fxrate_core::{FxRateSnapshot, FxRateSnapshotError, RoundingMode};
use money::{Currency, MonetaryAmount, Price};

/// Explicit asking-price assertion for a ProductListing.
///
/// `None` around this value means the source made no asking-price assertion.
#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductListingPrice {
    Monetary(Price),
    /// The source asks a price between two amounts, such as `€ 1.000 – 1.500`.
    Range(PriceRange),
    OnRequest,
}

impl ProductListingPrice {
    /// The one amount the source asks, if it asks exactly one.
    pub const fn monetary(self) -> Option<Price> {
        match self {
            Self::Monetary(price) => Some(price),
            Self::Range(_) | Self::OnRequest => None,
        }
    }

    /// The lowest and highest amount the source asks: both are the amount of a monetary price.
    /// `None` for a price on request, which has no amount.
    pub const fn bounds(self) -> Option<(Price, Price)> {
        match self {
            Self::Monetary(price) => Some((price, price)),
            Self::Range(range) => Some((range.min(), range.max())),
            Self::OnRequest => None,
        }
    }

    pub const fn is_on_request(self) -> bool {
        matches!(self, Self::OnRequest)
    }

    /// Converts every amount the price states into `currency` with `snapshot`. A range whose
    /// bounds round to one amount of the target currency becomes that amount.
    pub fn convert(
        self,
        snapshot: &FxRateSnapshot,
        currency: Currency,
        rounding: RoundingMode,
    ) -> Result<Self, FxRateSnapshotError> {
        let convert = |price| snapshot.convert(price, currency, rounding);
        Ok(match self {
            Self::Monetary(price) => Self::Monetary(convert(price)?),
            Self::Range(range) => {
                let (min, max) = (convert(range.min)?, convert(range.max)?);
                if min == max {
                    Self::Monetary(min)
                } else {
                    // Conversion keeps one currency and the order of the bounds, so only an
                    // upper bound beyond the storable minor units fails here.
                    Self::Range(
                        PriceRange::new(min, max)
                            .map_err(|_| FxRateSnapshotError::ConversionOverflow)?,
                    )
                }
            }
            Self::OnRequest => Self::OnRequest,
        })
    }
}

impl From<Price> for ProductListingPrice {
    fn from(value: Price) -> Self {
        Self::Monetary(value)
    }
}

impl From<PriceRange> for ProductListingPrice {
    fn from(value: PriceRange) -> Self {
        Self::Range(value)
    }
}

/// An asking price between two amounts of one currency.
///
/// The lower bound is strictly below the upper bound, and both fit the signed 64-bit minor
/// units prices persist as. [`PriceRange::new`] is the only constructor, so persisted, partner and
/// parsed ranges all pass the same validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceRange {
    min: Price,
    max: Price,
}

/// The largest amount, in minor units, a stored price can hold.
pub const MAX_STORABLE_MINOR_UNITS: u64 = i64::MAX as u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PriceRangeError {
    #[error("price range bounds must share one currency")]
    MixedCurrencies,
    #[error("price range lower bound must be below its upper bound")]
    NotAscending,
    #[error("price range bound exceeds the storable minor units")]
    BeyondStorableMinorUnits,
}

impl PriceRange {
    pub fn new(min: Price, max: Price) -> Result<Self, PriceRangeError> {
        if min.currency != max.currency {
            return Err(PriceRangeError::MixedCurrencies);
        }
        if *max.monetary_amount > MAX_STORABLE_MINOR_UNITS {
            return Err(PriceRangeError::BeyondStorableMinorUnits);
        }
        if min.monetary_amount >= max.monetary_amount {
            return Err(PriceRangeError::NotAscending);
        }
        Ok(Self { min, max })
    }

    /// Builds a range from minor-unit bounds in one currency.
    pub fn from_amounts(
        currency: Currency,
        min: impl Into<MonetaryAmount>,
        max: impl Into<MonetaryAmount>,
    ) -> Result<Self, PriceRangeError> {
        Self::new(
            Price::new(min.into(), currency),
            Price::new(max.into(), currency),
        )
    }

    pub const fn min(self) -> Price {
        self.min
    }

    pub const fn max(self) -> Price {
        self.max
    }

    pub const fn currency(self) -> Currency {
        self.min.currency
    }
}

#[cfg(feature = "test-data")]
impl fake::Dummy<fake::Faker> for PriceRange {
    fn dummy_with_rng<R: fake::RngExt + ?Sized>(_: &fake::Faker, rng: &mut R) -> Self {
        let currency: Currency = fake::Fake::fake_with_rng(&fake::Faker, rng);
        let min = rng.random_range(0..=1_000_000_000_u64);
        let max = min + rng.random_range(1..=1_000_000_000_u64);
        Self::from_amounts(currency, min, max).expect("generated bounds are ascending")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eur(amount: u64) -> Price {
        Price::new(MonetaryAmount::from(amount), Currency::Eur)
    }

    #[test]
    fn should_expose_monetary_and_on_request_helpers() {
        let price = eur(12_000);

        assert_eq!(Some(price), ProductListingPrice::from(price).monetary());
        assert_eq!(
            Some((price, price)),
            ProductListingPrice::from(price).bounds()
        );
        assert!(!ProductListingPrice::from(price).is_on_request());
        assert_eq!(None, ProductListingPrice::OnRequest.monetary());
        assert_eq!(None, ProductListingPrice::OnRequest.bounds());
        assert!(ProductListingPrice::OnRequest.is_on_request());
    }

    #[test]
    fn should_expose_range_bounds_but_no_single_amount() {
        let range = PriceRange::new(eur(100_000), eur(150_000)).expect("valid range");
        let price = ProductListingPrice::from(range);

        assert_eq!(None, price.monetary());
        assert_eq!(Some((eur(100_000), eur(150_000))), price.bounds());
        assert!(!price.is_on_request());
        assert_eq!(Currency::Eur, range.currency());
    }

    #[rstest::rstest]
    #[case::equal_bounds(eur(100), eur(100), PriceRangeError::NotAscending)]
    #[case::descending(eur(200), eur(100), PriceRangeError::NotAscending)]
    #[case::mixed_currencies(
        eur(100),
        Price::new(MonetaryAmount::from(200_u64), Currency::Usd),
        PriceRangeError::MixedCurrencies
    )]
    #[case::beyond_storable(
        eur(100),
        eur(MAX_STORABLE_MINOR_UNITS + 1),
        PriceRangeError::BeyondStorableMinorUnits
    )]
    fn should_reject_invalid_ranges_when_constructed(
        #[case] min: Price,
        #[case] max: Price,
        #[case] expected: PriceRangeError,
    ) {
        assert_eq!(Err(expected), PriceRange::new(min, max));
    }

    fn snapshot(usd_per_eur: u64) -> FxRateSnapshot {
        use fxrate_core::{FX_RATE_SCALE, FxRateId, FxRateQuote, FxRateSource, NewFxRateSnapshot};
        use strum::IntoEnumIterator;

        NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            time::OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| {
                FxRateQuote::new(
                    currency,
                    if currency == Currency::Usd {
                        usd_per_eur
                    } else {
                        FX_RATE_SCALE
                    },
                )
            }),
        )
        .map(|snapshot| snapshot.into_persisted(1_i64.try_into().expect("positive generation")))
        .expect("valid snapshot")
    }

    #[test]
    fn should_convert_each_bound_of_a_range() {
        let range = PriceRange::new(eur(100_000), eur(150_000)).expect("valid range");
        let snapshot = snapshot(2 * fxrate_core::FX_RATE_SCALE);

        assert_eq!(
            Ok(ProductListingPrice::from(
                PriceRange::from_amounts(Currency::Usd, 200_000_u64, 300_000_u64)
                    .expect("valid range")
            )),
            ProductListingPrice::from(range).convert(
                &snapshot,
                Currency::Usd,
                RoundingMode::HalfUp
            )
        );
        assert_eq!(
            Ok(ProductListingPrice::OnRequest),
            ProductListingPrice::OnRequest.convert(&snapshot, Currency::Usd, RoundingMode::HalfUp)
        );
    }

    #[test]
    fn should_present_one_amount_when_converted_bounds_round_together() {
        let range = PriceRange::new(eur(100), eur(101)).expect("valid range");
        let snapshot = snapshot(fxrate_core::FX_RATE_SCALE / 100);

        assert_eq!(
            Ok(ProductListingPrice::Monetary(Price::new(
                MonetaryAmount::from(1_u64),
                Currency::Usd
            ))),
            ProductListingPrice::from(range).convert(
                &snapshot,
                Currency::Usd,
                RoundingMode::HalfUp
            )
        );
    }

    #[test]
    fn should_accept_the_largest_storable_upper_bound() {
        assert!(PriceRange::new(eur(0), eur(MAX_STORABLE_MINOR_UNITS)).is_ok());
    }
}
