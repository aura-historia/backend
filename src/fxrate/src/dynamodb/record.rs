use common::{
    currency::domain::Currency,
    price::domain::{
        FX_RATE_SCALE, FixedFxRate, FxRate, MonetaryAmount, MonetaryAmountOverflowError, Rate,
    },
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use strum::IntoEnumIterator;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FxRatesRecord {
    pub pk: String,
    pub sk: String,

    #[serde(flatten)]
    pub rates: HashMap<String, Rate>,

    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

pub fn rate_key(from: &Currency, to: &Currency) -> String {
    format!(
        "{}_{}",
        from.as_str().to_lowercase(),
        to.as_str().to_lowercase()
    )
}

pub fn mk_pk() -> &'static str {
    "global#fx_rate"
}

pub fn mk_sk() -> &'static str {
    "fx_rate#details"
}

impl FxRatesRecord {
    fn get_rate(&self, from: Currency, to: Currency) -> Rate {
        if from == to {
            return FX_RATE_SCALE;
        }
        self.rates
            .get(&rate_key(&from, &to))
            .copied()
            .unwrap_or(FX_RATE_SCALE)
    }
}

impl FxRate for FxRatesRecord {
    fn exchange(
        &self,
        from_currency: Currency,
        to_currency: Currency,
        from_amount: MonetaryAmount,
    ) -> Result<MonetaryAmount, MonetaryAmountOverflowError> {
        let rate = self.get_rate(from_currency, to_currency);

        // Half-Up Rounding
        let numerator = (*from_amount)
            .checked_mul(rate)
            .ok_or(MonetaryAmountOverflowError)?;
        let half = FX_RATE_SCALE / 2;
        let converted = (numerator + half) / FX_RATE_SCALE;

        Ok(MonetaryAmount::from(converted))
    }
}

impl From<FixedFxRate> for FxRatesRecord {
    fn from(value: FixedFxRate) -> Self {
        let mut rates = HashMap::new();
        for src in Currency::iter() {
            for tgt in Currency::iter() {
                if src != tgt {
                    rates.insert(rate_key(&src, &tgt), value.get_rate(src, tgt));
                }
            }
        }
        FxRatesRecord {
            pk: mk_pk().to_owned(),
            sk: mk_sk().to_owned(),
            rates,
            timestamp: OffsetDateTime::now_utc(),
        }
    }
}
