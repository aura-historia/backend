use crate::{MonetaryAmount, Price};
use std::collections::HashMap;

#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub struct MinorUnitExponent(pub u8);

impl From<u8> for MinorUnitExponent {
    fn from(value: u8) -> Self {
        Self(value)
    }
}

impl From<MinorUnitExponent> for u8 {
    fn from(value: MinorUnitExponent) -> Self {
        value.0
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
    Zar,
    Sek,
    Dkk,
    Nok,
    Krw,
    Inr,
    Twd,
    Huf,
    Ron,
    Mxn,
    Thb,
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

    pub fn extract_amount(
        self,
        native: &Option<Price>,
        other: &HashMap<Currency, MonetaryAmount>,
    ) -> Option<MonetaryAmount> {
        other.get(&self).copied().or_else(|| {
            native
                .filter(|price| price.currency == self)
                .map(|price| price.monetary_amount)
        })
    }

    pub fn currency_symbol(self) -> &'static str {
        self.display_properties().0
    }

    pub fn decimal_separator(self) -> &'static str {
        self.display_properties().1
    }

    pub fn is_leading_sign(self) -> bool {
        self.display_properties().2
    }

    fn display_properties(self) -> (&'static str, &'static str, bool) {
        match self {
            Currency::Eur => ("€", ",", false),
            Currency::Gbp => ("£", ".", true),
            Currency::Usd => ("$", ".", true),
            Currency::Aud => ("A$", ".", true),
            Currency::Cad => ("C$", ".", true),
            Currency::Nzd => ("NZ$", ".", true),
            Currency::Cny => ("CN¥", ".", true),
            Currency::Brl => ("R$", ",", true),
            Currency::Pln => ("zł", ",", false),
            Currency::Try => ("₺", ",", false),
            Currency::Jpy => ("¥", ".", true),
            Currency::Czk => ("Kč", ",", false),
            Currency::Rub => ("₽", ",", false),
            Currency::Aed => ("د.إ", ".", false),
            Currency::Sar => ("﷼", ".", false),
            Currency::Hkd => ("HK$", ".", true),
            Currency::Sgd => ("S$", ".", true),
            Currency::Chf => ("CHF", ".", true),
            Currency::Zar => ("R", ".", true),
            Currency::Sek => ("SEK", ",", false),
            Currency::Dkk => ("DKK", ",", false),
            Currency::Nok => ("NOK", ",", false),
            Currency::Krw => ("₩", ".", true),
            Currency::Inr => ("₹", ".", true),
            Currency::Twd => ("NT$", ".", true),
            Currency::Huf => ("Ft", ",", false),
            Currency::Ron => ("lei", ",", false),
            Currency::Mxn => ("MX$", ".", true),
            Currency::Thb => ("฿", ".", true),
        }
    }

    pub fn from_code(value: &str) -> Option<Self> {
        use strum::IntoEnumIterator;

        Self::iter().find(|currency| currency.as_str() == value)
    }

    pub fn as_str(self) -> &'static str {
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
            Currency::Zar => "ZAR",
            Currency::Sek => "SEK",
            Currency::Dkk => "DKK",
            Currency::Nok => "NOK",
            Currency::Krw => "KRW",
            Currency::Inr => "INR",
            Currency::Twd => "TWD",
            Currency::Huf => "HUF",
            Currency::Ron => "RON",
            Currency::Mxn => "MXN",
            Currency::Thb => "THB",
        }
    }
}

pub trait HasMinorUnitExponent {
    fn minor_unit_exponent(&self) -> MinorUnitExponent;
}

impl HasMinorUnitExponent for Currency {
    fn minor_unit_exponent(&self) -> MinorUnitExponent {
        match self {
            Currency::Jpy | Currency::Krw => MinorUnitExponent(0),
            _ => MinorUnitExponent(2),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;

    #[test]
    fn should_round_trip_all_canonical_currency_codes() {
        for currency in Currency::iter() {
            assert_eq!(Some(currency), Currency::from_code(currency.as_str()));
        }
        assert_eq!(None, Currency::from_code("eur"));
    }
    #[test]
    fn should_format_supported_currencies_with_iso_minor_units() {
        for (currency, exponent, expected) in [
            (Currency::Eur, 2, "123,45 €"),
            (Currency::Gbp, 2, "£123.45"),
            (Currency::Usd, 2, "$123.45"),
            (Currency::Aud, 2, "A$123.45"),
            (Currency::Cad, 2, "C$123.45"),
            (Currency::Nzd, 2, "NZ$123.45"),
            (Currency::Cny, 2, "CN¥123.45"),
            (Currency::Brl, 2, "R$123,45"),
            (Currency::Pln, 2, "123,45 zł"),
            (Currency::Try, 2, "123,45 ₺"),
            (Currency::Jpy, 0, "¥12345"),
            (Currency::Czk, 2, "123,45 Kč"),
            (Currency::Rub, 2, "123,45 ₽"),
            (Currency::Aed, 2, "123.45 د.إ"),
            (Currency::Sar, 2, "123.45 ﷼"),
            (Currency::Hkd, 2, "HK$123.45"),
            (Currency::Sgd, 2, "S$123.45"),
            (Currency::Chf, 2, "CHF123.45"),
            (Currency::Zar, 2, "R123.45"),
            (Currency::Sek, 2, "123,45 SEK"),
            (Currency::Dkk, 2, "123,45 DKK"),
            (Currency::Nok, 2, "123,45 NOK"),
            (Currency::Krw, 0, "₩12345"),
            (Currency::Inr, 2, "₹123.45"),
            (Currency::Twd, 2, "NT$123.45"),
            (Currency::Huf, 2, "123,45 Ft"),
            (Currency::Ron, 2, "123,45 lei"),
            (Currency::Mxn, 2, "MX$123.45"),
            (Currency::Thb, 2, "฿123.45"),
        ] {
            assert_eq!(MinorUnitExponent(exponent), currency.minor_unit_exponent());
            assert_eq!(
                expected,
                Price::new(12345_u64.into(), currency).format_human_readable()
            );
        }
    }
}
