use common::{
    currency::domain::Currency, fx_rate_id::FxRateId, price::domain::MonetaryAmount,
    query::range_query::RangeQuery,
};
use fxrate_core::{DisplayAmountRange, FxRateSnapshot, FxRateSnapshotError, RoundingMode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductPriceFilterPlan {
    pub fx_rate_id: FxRateId,
    pub target_currency: Currency,
    pub active_native_ranges: Vec<NativePriceRange>,
    pub sold_display_range: RangeQuery<MonetaryAmount>,
    has_price_filter: bool,
    snapshot: FxRateSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativePriceRange {
    pub source_currency: Currency,
    pub lower: u64,
    pub upper: Option<u64>,
}

impl ProductPriceFilterPlan {
    pub fn compile(
        snapshot: FxRateSnapshot,
        target_currency: Currency,
        sold_display_range: Option<RangeQuery<MonetaryAmount>>,
    ) -> Result<Self, FxRateSnapshotError> {
        let has_price_filter = sold_display_range.is_some();
        let sold_display_range = sold_display_range.unwrap_or(RangeQuery {
            min: None,
            max: None,
        });
        let display_range = DisplayAmountRange {
            lower: sold_display_range.min,
            upper: sold_display_range.max,
        };
        let active_native_ranges = snapshot
            .quotes()
            .iter()
            .map(|quote| {
                snapshot
                    .compile_source_range(quote.currency(), target_currency, display_range)
                    .map(|range| {
                        range.map(|range| NativePriceRange {
                            source_currency: quote.currency(),
                            lower: range.lower.into(),
                            upper: range.upper.map(Into::into),
                        })
                    })
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        Ok(Self {
            fx_rate_id: snapshot.id(),
            target_currency,
            active_native_ranges,
            sold_display_range,
            has_price_filter,
            snapshot,
        })
    }

    pub fn has_price_filter(&self) -> bool {
        self.has_price_filter
    }

    pub fn captured_at(&self) -> time::OffsetDateTime {
        self.snapshot.captured_at()
    }

    pub fn convert_active_source_amount(
        &self,
        source_currency: Currency,
        source_amount: u64,
    ) -> Result<u64, FxRateSnapshotError> {
        self.snapshot
            .convert(
                common::price::domain::Price::new(source_amount.into(), source_currency),
                self.target_currency,
                RoundingMode::HalfUp,
            )
            .map(|price| price.monetary_amount.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxrate_core::{FX_RATE_SCALE, FxRateQuote, FxRateSource, NewFxRateSnapshot};
    use strum::IntoEnumIterator;
    use time::OffsetDateTime;

    fn snapshot() -> Result<FxRateSnapshot, FxRateSnapshotError> {
        NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| {
                FxRateQuote::new(
                    currency,
                    match currency {
                        Currency::Eur => FX_RATE_SCALE,
                        Currency::Usd => 1_100_000,
                        Currency::Jpy => 160_000_000,
                        _ => FX_RATE_SCALE,
                    },
                )
            }),
        )
        .and_then(|snapshot| Ok(snapshot.into_persisted(1_i64.try_into()?)))
    }

    #[test]
    fn should_compile_exact_native_ranges_for_a_pinned_snapshot()
    -> Result<(), Box<dyn std::error::Error>> {
        let snapshot = snapshot()?;

        let plan = ProductPriceFilterPlan::compile(
            snapshot.clone(),
            Currency::Usd,
            Some(RangeQuery {
                min: Some(110_u64.into()),
                max: Some(110_u64.into()),
            }),
        )?;

        assert_eq!(snapshot.id(), plan.fx_rate_id);
        assert_eq!(Currency::Usd, plan.target_currency);
        assert_eq!(Some(110_u64.into()), plan.sold_display_range.min);
        assert_eq!(Some(110_u64.into()), plan.sold_display_range.max);
        assert!(plan.has_price_filter());
        assert!(plan.active_native_ranges.iter().any(|range| {
            range.source_currency == Currency::Eur && range.lower == 100 && range.upper == Some(100)
        }));
        assert_eq!(110, plan.convert_active_source_amount(Currency::Eur, 100)?);
        Ok(())
    }

    #[test]
    fn should_compile_jpy_native_range_for_two_decimal_usd_display_price()
    -> Result<(), Box<dyn std::error::Error>> {
        let plan = ProductPriceFilterPlan::compile(
            snapshot()?,
            Currency::Usd,
            Some(RangeQuery {
                min: Some(110_u64.into()),
                max: Some(110_u64.into()),
            }),
        )?;

        assert!(plan.active_native_ranges.iter().any(|range| {
            range.source_currency == Currency::Jpy && range.lower == 160 && range.upper == Some(160)
        }));
        assert_eq!(110, plan.convert_active_source_amount(Currency::Jpy, 160)?);
        Ok(())
    }

    #[test]
    fn should_preserve_open_display_bounds_in_native_ranges()
    -> Result<(), Box<dyn std::error::Error>> {
        let lower_open = ProductPriceFilterPlan::compile(
            snapshot()?,
            Currency::Usd,
            Some(RangeQuery {
                min: None,
                max: Some(110_u64.into()),
            }),
        )?;
        let upper_open = ProductPriceFilterPlan::compile(
            snapshot()?,
            Currency::Usd,
            Some(RangeQuery {
                min: Some(110_u64.into()),
                max: None,
            }),
        )?;

        assert!(lower_open.active_native_ranges.iter().any(|range| {
            range.source_currency == Currency::Eur && range.lower == 0 && range.upper == Some(100)
        }));
        assert!(upper_open.active_native_ranges.iter().any(|range| {
            range.source_currency == Currency::Eur && range.lower == 100 && range.upper.is_none()
        }));
        Ok(())
    }

    #[test]
    fn should_keep_snapshot_when_no_price_range_is_requested()
    -> Result<(), Box<dyn std::error::Error>> {
        let snapshot = snapshot()?;
        let plan = ProductPriceFilterPlan::compile(snapshot.clone(), Currency::Usd, None)?;

        assert_eq!(snapshot.id(), plan.fx_rate_id);
        assert!(!plan.has_price_filter());
        Ok(())
    }

    #[test]
    fn should_mark_active_prices_unmatchable_when_display_range_is_invalid()
    -> Result<(), Box<dyn std::error::Error>> {
        let plan = ProductPriceFilterPlan::compile(
            snapshot()?,
            Currency::Eur,
            Some(RangeQuery {
                min: Some(200_u64.into()),
                max: Some(100_u64.into()),
            }),
        )?;

        assert!(plan.active_native_ranges.is_empty());
        Ok(())
    }
}
