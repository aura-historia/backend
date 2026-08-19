use crate::currency_record::CurrencyRecord;
use money::Price;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct PriceRecord {
    currency: CurrencyRecord,
    amount: u64,
}

impl From<Price> for PriceRecord {
    fn from(value: Price) -> Self {
        Self {
            currency: value.currency.into(),
            amount: value.monetary_amount.into(),
        }
    }
}

impl From<PriceRecord> for Price {
    fn from(value: PriceRecord) -> Self {
        Self::new(value.amount.into(), value.currency.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use money::{Currency, MonetaryAmount};

    #[test]
    fn should_preserve_native_price_dynamodb_strings() -> Result<(), serde_json::Error> {
        let record = PriceRecord::from(Price::new(MonetaryAmount::from(542_u64), Currency::Eur));

        assert_eq!(
            serde_json::to_string(&record)?,
            r#"{"currency":"EUR","amount":542}"#
        );

        Ok(())
    }
}
