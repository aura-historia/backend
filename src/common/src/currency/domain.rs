use crate::currency::data::CurrencyData;
use crate::currency::record::CurrencyRecord;
use crate::price::domain::{MonetaryAmount, Price};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub struct MinorUnitExponent(pub u8);

impl From<u8> for MinorUnitExponent {
    fn from(item: u8) -> Self {
        Self(item)
    }
}

impl From<MinorUnitExponent> for u8 {
    fn from(item: MinorUnitExponent) -> Self {
        item.0
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(
    Copy,
    Clone,
    Eq,
    PartialEq,
    Debug,
    Default,
    Hash,
    strum_macros::EnumIter,
    strum_macros::Display,
    strum_macros::EnumCount,
    Serialize,
    Deserialize,
)]
pub enum Currency {
    #[default]
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

impl Currency {
    pub fn resolve(
        preferred: &[Currency],
        available: HashMap<Currency, MonetaryAmount>,
    ) -> Option<Price> {
        let mut available = available;
        preferred
            .iter()
            .find_map(|currency| {
                available
                    .remove(currency)
                    .map(|amount| Price::new(amount, *currency))
            })
            .or_else(|| {
                available
                    .remove(&Currency::Eur)
                    .map(|amount| Price::new(amount, Currency::Eur))
            })
            .or_else(|| {
                available
                    .remove(&Currency::Usd)
                    .map(|amount| Price::new(amount, Currency::Usd))
            })
            .or_else(|| {
                available
                    .remove(&Currency::Gbp)
                    .map(|amount| Price::new(amount, Currency::Gbp))
            })
            .or_else(|| {
                available
                    .into_iter()
                    .next()
                    .map(|(currency, amount)| Price::new(amount, currency))
            })
    }

    /// Extracts the [`MonetaryAmount`] for the given [`Currency`] from a combined
    /// native-price / other-price pair. Prefers the `other` map and falls back
    /// to `native` when its currency matches.
    pub fn extract_amount(
        self,
        native: &Option<Price>,
        other: &HashMap<Currency, MonetaryAmount>,
    ) -> Option<MonetaryAmount> {
        if let Some(amount) = other.get(&self) {
            return Some(*amount);
        }
        native
            .filter(|p| p.currency == self)
            .map(|p| p.monetary_amount)
    }

    pub fn currency_symbol(&self) -> &'static str {
        match self {
            Currency::Eur => "€",
            Currency::Gbp => "£",
            Currency::Usd => "$",
            Currency::Aud => "A$",
            Currency::Cad => "C$",
            Currency::Nzd => "NZ$",
            Currency::Cny => "CN¥",
            Currency::Brl => "R$",
            Currency::Pln => "zł",
            Currency::Try => "₺",
            Currency::Jpy => "¥",
            Currency::Czk => "Kč",
            Currency::Rub => "₽",
            Currency::Aed => "د.إ",
            Currency::Sar => "﷼",
            Currency::Hkd => "HK$",
            Currency::Sgd => "S$",
            Currency::Chf => "CHF",
        }
    }

    pub fn decimal_separator(&self) -> &'static str {
        match self {
            Currency::Eur
            | Currency::Brl
            | Currency::Pln
            | Currency::Try
            | Currency::Czk
            | Currency::Rub => ",",
            Currency::Gbp
            | Currency::Usd
            | Currency::Aud
            | Currency::Cad
            | Currency::Nzd
            | Currency::Cny
            | Currency::Jpy
            | Currency::Aed
            | Currency::Sar
            | Currency::Hkd
            | Currency::Sgd
            | Currency::Chf => ".",
        }
    }

    pub fn is_leading_sign(&self) -> bool {
        match self {
            Currency::Eur
            | Currency::Pln
            | Currency::Try
            | Currency::Czk
            | Currency::Rub
            | Currency::Aed
            | Currency::Sar => false,
            Currency::Gbp
            | Currency::Usd
            | Currency::Aud
            | Currency::Cad
            | Currency::Nzd
            | Currency::Cny
            | Currency::Brl
            | Currency::Jpy
            | Currency::Hkd
            | Currency::Sgd
            | Currency::Chf => true,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Currency::Eur => "EUR",
            Currency::Gbp => "GBP",
            Currency::Usd => "USD",
            Currency::Aud => "AUD",
            Currency::Cad => "CAD",
            Currency::Nzd => "NZD",
            Currency::Cny => "CNY",
            Currency::Brl => "BRL",
            Currency::Pln => "PLN",
            Currency::Try => "TRY",
            Currency::Jpy => "JPY",
            Currency::Czk => "CZK",
            Currency::Rub => "RUB",
            Currency::Aed => "AED",
            Currency::Sar => "SAR",
            Currency::Hkd => "HKD",
            Currency::Sgd => "SGD",
            Currency::Chf => "CHF",
        }
    }
}

pub trait HasMinorUnitExponent {
    fn minor_unit_exponent(&self) -> MinorUnitExponent;
}

impl HasMinorUnitExponent for Currency {
    fn minor_unit_exponent(&self) -> MinorUnitExponent {
        match self {
            Currency::Jpy => MinorUnitExponent(0),
            Currency::Eur
            | Currency::Gbp
            | Currency::Usd
            | Currency::Aud
            | Currency::Cad
            | Currency::Nzd
            | Currency::Cny
            | Currency::Brl
            | Currency::Pln
            | Currency::Try
            | Currency::Czk
            | Currency::Rub
            | Currency::Aed
            | Currency::Sar
            | Currency::Hkd
            | Currency::Sgd
            | Currency::Chf => MinorUnitExponent(2),
        }
    }
}

impl From<CurrencyRecord> for Currency {
    fn from(cmd: CurrencyRecord) -> Self {
        match cmd {
            CurrencyRecord::Eur => Currency::Eur,
            CurrencyRecord::Gbp => Currency::Gbp,
            CurrencyRecord::Usd => Currency::Usd,
            CurrencyRecord::Aud => Currency::Aud,
            CurrencyRecord::Cad => Currency::Cad,
            CurrencyRecord::Nzd => Currency::Nzd,
            CurrencyRecord::Cny => Currency::Cny,
            CurrencyRecord::Brl => Currency::Brl,
            CurrencyRecord::Pln => Currency::Pln,
            CurrencyRecord::Try => Currency::Try,
            CurrencyRecord::Jpy => Currency::Jpy,
            CurrencyRecord::Czk => Currency::Czk,
            CurrencyRecord::Rub => Currency::Rub,
            CurrencyRecord::Aed => Currency::Aed,
            CurrencyRecord::Sar => Currency::Sar,
            CurrencyRecord::Hkd => Currency::Hkd,
            CurrencyRecord::Sgd => Currency::Sgd,
            CurrencyRecord::Chf => Currency::Chf,
        }
    }
}

impl From<CurrencyData> for Currency {
    fn from(data: CurrencyData) -> Self {
        match data {
            CurrencyData::Eur => Currency::Eur,
            CurrencyData::Gbp => Currency::Gbp,
            CurrencyData::Usd => Currency::Usd,
            CurrencyData::Aud => Currency::Aud,
            CurrencyData::Cad => Currency::Cad,
            CurrencyData::Nzd => Currency::Nzd,
            CurrencyData::Cny => Currency::Cny,
            CurrencyData::Brl => Currency::Brl,
            CurrencyData::Pln => Currency::Pln,
            CurrencyData::Try => Currency::Try,
            CurrencyData::Jpy => Currency::Jpy,
            CurrencyData::Czk => Currency::Czk,
            CurrencyData::Rub => Currency::Rub,
            CurrencyData::Aed => Currency::Aed,
            CurrencyData::Sar => Currency::Sar,
            CurrencyData::Hkd => Currency::Hkd,
            CurrencyData::Sgd => Currency::Sgd,
            CurrencyData::Chf => Currency::Chf,
        }
    }
}
