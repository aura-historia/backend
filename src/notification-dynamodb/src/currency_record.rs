use money::Currency;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum CurrencyRecord {
    Eur,
    Gbp,
    Usd,
    Aud,
    Cad,
    Nzd,
    Cny,
    Brl,
    Pln,
    Try,
    Jpy,
    Czk,
    Rub,
    Aed,
    Sar,
    Hkd,
    Sgd,
    Chf,
}

impl From<Currency> for CurrencyRecord {
    fn from(value: Currency) -> Self {
        match value {
            Currency::Eur => Self::Eur,
            Currency::Gbp => Self::Gbp,
            Currency::Usd => Self::Usd,
            Currency::Aud => Self::Aud,
            Currency::Cad => Self::Cad,
            Currency::Nzd => Self::Nzd,
            Currency::Cny => Self::Cny,
            Currency::Brl => Self::Brl,
            Currency::Pln => Self::Pln,
            Currency::Try => Self::Try,
            Currency::Jpy => Self::Jpy,
            Currency::Czk => Self::Czk,
            Currency::Rub => Self::Rub,
            Currency::Aed => Self::Aed,
            Currency::Sar => Self::Sar,
            Currency::Hkd => Self::Hkd,
            Currency::Sgd => Self::Sgd,
            Currency::Chf => Self::Chf,
        }
    }
}

impl From<CurrencyRecord> for Currency {
    fn from(value: CurrencyRecord) -> Self {
        match value {
            CurrencyRecord::Eur => Self::Eur,
            CurrencyRecord::Gbp => Self::Gbp,
            CurrencyRecord::Usd => Self::Usd,
            CurrencyRecord::Aud => Self::Aud,
            CurrencyRecord::Cad => Self::Cad,
            CurrencyRecord::Nzd => Self::Nzd,
            CurrencyRecord::Cny => Self::Cny,
            CurrencyRecord::Brl => Self::Brl,
            CurrencyRecord::Pln => Self::Pln,
            CurrencyRecord::Try => Self::Try,
            CurrencyRecord::Jpy => Self::Jpy,
            CurrencyRecord::Czk => Self::Czk,
            CurrencyRecord::Rub => Self::Rub,
            CurrencyRecord::Aed => Self::Aed,
            CurrencyRecord::Sar => Self::Sar,
            CurrencyRecord::Hkd => Self::Hkd,
            CurrencyRecord::Sgd => Self::Sgd,
            CurrencyRecord::Chf => Self::Chf,
        }
    }
}
