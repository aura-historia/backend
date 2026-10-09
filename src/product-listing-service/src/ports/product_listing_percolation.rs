use super::ProductListingSearchFilterMatchSource;
use fxrate_core::{FxRateId, FxRateSnapshot, FxRateSnapshotError, RoundingMode};
use money::Currency;
use money::Price;
use product_listing_core::product_listing::ProductListingPriceValuationBasis;
use time::OffsetDateTime;

/// Display prices from one validated snapshot for a temporary percolation input.
/// Expanded currencies are absent together for immutable legacy snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductListingPricesByCurrency {
    eur: u64,
    gbp: u64,
    usd: u64,
    aud: u64,
    cad: u64,
    nzd: u64,
    cny: u64,
    brl: u64,
    pln: u64,
    r#try: u64,
    jpy: u64,
    czk: u64,
    rub: u64,
    aed: u64,
    sar: u64,
    hkd: u64,
    sgd: u64,
    chf: u64,
    zar: u64,
    sek: Option<u64>,
    dkk: Option<u64>,
    nok: Option<u64>,
    krw: Option<u64>,
    inr: Option<u64>,
    twd: Option<u64>,
    huf: Option<u64>,
    ron: Option<u64>,
    mxn: Option<u64>,
    thb: Option<u64>,
}

impl ProductListingPricesByCurrency {
    pub fn convert_all(
        snapshot: &FxRateSnapshot,
        source_price: Price,
    ) -> Result<Self, FxRateSnapshotError> {
        let amount_in = |currency| {
            snapshot
                .convert(source_price, currency, RoundingMode::HalfUp)
                .map(|price| u64::from(price.monetary_amount))
        };

        let optional_amount_in = |currency| match amount_in(currency) {
            Ok(amount) => Ok(Some(amount)),
            Err(FxRateSnapshotError::MissingQuote(missing)) if missing == currency => Ok(None),
            Err(error) => Err(error),
        };

        Ok(Self {
            eur: amount_in(Currency::Eur)?,
            gbp: amount_in(Currency::Gbp)?,
            usd: amount_in(Currency::Usd)?,
            aud: amount_in(Currency::Aud)?,
            cad: amount_in(Currency::Cad)?,
            nzd: amount_in(Currency::Nzd)?,
            cny: amount_in(Currency::Cny)?,
            brl: amount_in(Currency::Brl)?,
            pln: amount_in(Currency::Pln)?,
            r#try: amount_in(Currency::Try)?,
            jpy: amount_in(Currency::Jpy)?,
            czk: amount_in(Currency::Czk)?,
            rub: amount_in(Currency::Rub)?,
            aed: amount_in(Currency::Aed)?,
            sar: amount_in(Currency::Sar)?,
            hkd: amount_in(Currency::Hkd)?,
            sgd: amount_in(Currency::Sgd)?,
            chf: amount_in(Currency::Chf)?,
            zar: amount_in(Currency::Zar)?,
            sek: optional_amount_in(Currency::Sek)?,
            dkk: optional_amount_in(Currency::Dkk)?,
            nok: optional_amount_in(Currency::Nok)?,
            krw: optional_amount_in(Currency::Krw)?,
            inr: optional_amount_in(Currency::Inr)?,
            twd: optional_amount_in(Currency::Twd)?,
            huf: optional_amount_in(Currency::Huf)?,
            ron: optional_amount_in(Currency::Ron)?,
            mxn: optional_amount_in(Currency::Mxn)?,
            thb: optional_amount_in(Currency::Thb)?,
        })
    }

    pub fn amount_in(self, currency: Currency) -> Option<u64> {
        match currency {
            Currency::Eur => Some(self.eur),
            Currency::Gbp => Some(self.gbp),
            Currency::Usd => Some(self.usd),
            Currency::Aud => Some(self.aud),
            Currency::Cad => Some(self.cad),
            Currency::Nzd => Some(self.nzd),
            Currency::Cny => Some(self.cny),
            Currency::Brl => Some(self.brl),
            Currency::Pln => Some(self.pln),
            Currency::Try => Some(self.r#try),
            Currency::Jpy => Some(self.jpy),
            Currency::Czk => Some(self.czk),
            Currency::Rub => Some(self.rub),
            Currency::Aed => Some(self.aed),
            Currency::Sar => Some(self.sar),
            Currency::Hkd => Some(self.hkd),
            Currency::Sgd => Some(self.sgd),
            Currency::Chf => Some(self.chf),
            Currency::Zar => Some(self.zar),
            Currency::Sek => self.sek,
            Currency::Dkk => self.dkk,
            Currency::Nok => self.nok,
            Currency::Krw => self.krw,
            Currency::Inr => self.inr,
            Currency::Twd => self.twd,
            Currency::Huf => self.huf,
            Currency::Ron => self.ron,
            Currency::Mxn => self.mxn,
            Currency::Thb => self.thb,
        }
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
