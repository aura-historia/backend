use super::ProductListingSearchFilterMatchSource;
use fxrate_core::{FxRateId, FxRateSnapshot, FxRateSnapshotError, RoundingMode};
use money::Currency;
use money::Price;
use product_listing_core::product_listing::ProductListingPriceValuationBasis;
use time::OffsetDateTime;

/// Display prices from one validated snapshot for a temporary percolation input.
/// Expanded currencies are absent together for immutable legacy snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingPricesByCurrency {
    amounts: std::collections::HashMap<Currency, u64>,
}

impl ProductListingPricesByCurrency {
    pub fn convert_all(
        snapshot: &FxRateSnapshot,
        source_price: Price,
    ) -> Result<Self, FxRateSnapshotError> {
        let amounts = snapshot
            .quotes()
            .iter()
            .map(|quote| {
                let currency = quote.currency();
                snapshot
                    .convert(source_price, currency, RoundingMode::HalfUp)
                    .map(|price| (currency, u64::from(price.monetary_amount)))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { amounts })
    }

    pub fn amount_in(&self, currency: Currency) -> Option<u64> {
        self.amounts.get(&currency).copied()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingPercolationValuation {
    pub basis: ProductListingPriceValuationBasis,
    pub fx_rate_id: FxRateId,
    pub effective_at: OffsetDateTime,
    pub prices: ProductListingPricesByCurrency,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingPercolationInput {
    pub source: ProductListingSearchFilterMatchSource,
    /// Absent only when the ProductListing has no native main price.
    pub valuation: Option<ProductListingPercolationValuation>,
}
