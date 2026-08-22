use money::Currency;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Data-transfer object for [`Currency`] used in LLM-facing JSON schemas.
#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum CurrencyDto {
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

impl From<CurrencyDto> for Currency {
    fn from(dto: CurrencyDto) -> Self {
        match dto {
            CurrencyDto::Eur => Currency::Eur,
            CurrencyDto::Gbp => Currency::Gbp,
            CurrencyDto::Usd => Currency::Usd,
            CurrencyDto::Aud => Currency::Aud,
            CurrencyDto::Cad => Currency::Cad,
            CurrencyDto::Nzd => Currency::Nzd,
            CurrencyDto::Cny => Currency::Cny,
            CurrencyDto::Brl => Currency::Brl,
            CurrencyDto::Pln => Currency::Pln,
            CurrencyDto::Try => Currency::Try,
            CurrencyDto::Jpy => Currency::Jpy,
            CurrencyDto::Czk => Currency::Czk,
            CurrencyDto::Rub => Currency::Rub,
            CurrencyDto::Aed => Currency::Aed,
            CurrencyDto::Sar => Currency::Sar,
            CurrencyDto::Hkd => Currency::Hkd,
            CurrencyDto::Sgd => Currency::Sgd,
            CurrencyDto::Chf => Currency::Chf,
        }
    }
}

impl From<Currency> for CurrencyDto {
    fn from(c: Currency) -> Self {
        match c {
            Currency::Eur => CurrencyDto::Eur,
            Currency::Gbp => CurrencyDto::Gbp,
            Currency::Usd => CurrencyDto::Usd,
            Currency::Aud => CurrencyDto::Aud,
            Currency::Cad => CurrencyDto::Cad,
            Currency::Nzd => CurrencyDto::Nzd,
            Currency::Cny => CurrencyDto::Cny,
            Currency::Brl => CurrencyDto::Brl,
            Currency::Pln => CurrencyDto::Pln,
            Currency::Try => CurrencyDto::Try,
            Currency::Jpy => CurrencyDto::Jpy,
            Currency::Czk => CurrencyDto::Czk,
            Currency::Rub => CurrencyDto::Rub,
            Currency::Aed => CurrencyDto::Aed,
            Currency::Sar => CurrencyDto::Sar,
            Currency::Hkd => CurrencyDto::Hkd,
            Currency::Sgd => CurrencyDto::Sgd,
            Currency::Chf => CurrencyDto::Chf,
        }
    }
}
