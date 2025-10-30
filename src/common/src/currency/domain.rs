use crate::currency::data::CurrencyData;
use crate::currency::record::CurrencyRecord;
use crate::price::domain::{MonetaryAmount, Price};
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
)]
pub enum Currency {
    #[default]
    Eur,
    Gbp,
    Usd,
    Aud,
    Cad,
    Nzd,
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

    pub fn currency_symbol(&self) -> &'static str {
        match self {
            Currency::Eur => "€",
            Currency::Gbp => "£",
            Currency::Usd => "$",
            Currency::Aud => "A$",
            Currency::Cad => "C$",
            Currency::Nzd => "NZ$",
        }
    }

    pub fn decimal_separator(&self) -> &'static str {
        match self {
            Currency::Eur => ",",
            Currency::Gbp => ".",
            Currency::Usd => ".",
            Currency::Aud => ".",
            Currency::Cad => ".",
            Currency::Nzd => ".",
        }
    }

    pub fn is_leading_sign(&self) -> bool {
        match self {
            Currency::Eur => false,
            Currency::Gbp => true,
            Currency::Usd => true,
            Currency::Aud => true,
            Currency::Cad => true,
            Currency::Nzd => true,
        }
    }
}

pub trait HasMinorUnitExponent {
    fn minor_unit_exponent(&self) -> MinorUnitExponent;
}

impl HasMinorUnitExponent for Currency {
    fn minor_unit_exponent(&self) -> MinorUnitExponent {
        match self {
            Currency::Eur => MinorUnitExponent(2),
            Currency::Gbp => MinorUnitExponent(2),
            Currency::Usd => MinorUnitExponent(2),
            Currency::Aud => MinorUnitExponent(2),
            Currency::Cad => MinorUnitExponent(2),
            Currency::Nzd => MinorUnitExponent(2),
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
        }
    }
}
