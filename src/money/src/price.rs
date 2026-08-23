use crate::{Currency, HasMinorUnitExponent};
use std::ops::{Add, Deref, Sub};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MonetaryAmount(#[cfg_attr(feature = "test-data", dummy(faker = "0..=1000000000"))] u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, thiserror::Error)]
#[error("Monetary amount cannot be negative.")]
pub struct NegativeMonetaryAmountError;

impl Deref for MonetaryAmount {
    type Target = u64;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Add for MonetaryAmount {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl Sub for MonetaryAmount {
    type Output = Result<Self, NegativeMonetaryAmountError>;

    fn sub(self, rhs: Self) -> Self::Output {
        self.0
            .checked_sub(rhs.0)
            .map(Self)
            .ok_or(NegativeMonetaryAmountError)
    }
}

impl From<u8> for MonetaryAmount {
    fn from(value: u8) -> Self {
        Self(value.into())
    }
}

impl From<u16> for MonetaryAmount {
    fn from(value: u16) -> Self {
        Self(value.into())
    }
}

impl From<u32> for MonetaryAmount {
    fn from(value: u32) -> Self {
        Self(value.into())
    }
}

impl From<u64> for MonetaryAmount {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl From<MonetaryAmount> for u64 {
    fn from(value: MonetaryAmount) -> Self {
        value.0
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Price {
    pub monetary_amount: MonetaryAmount,
    pub currency: Currency,
}

impl Price {
    pub fn new(monetary_amount: MonetaryAmount, currency: Currency) -> Self {
        Self {
            monetary_amount,
            currency,
        }
    }

    pub fn format_human_readable(self) -> String {
        let exponent = self.currency.minor_unit_exponent().0 as u32;
        let currency_symbol = self.currency.currency_symbol();

        if exponent == 0 {
            let majors = self.monetary_amount.0;
            return if self.currency.is_leading_sign() {
                format!("{currency_symbol}{majors}")
            } else {
                format!("{majors} {currency_symbol}")
            };
        }

        let divisor = 10u64.pow(exponent);
        let majors = self.monetary_amount.0 / divisor;
        let minors = self.monetary_amount.0 % divisor;
        let width = exponent as usize;
        let decimal_separator = self.currency.decimal_separator();

        if self.currency.is_leading_sign() {
            format!("{currency_symbol}{majors}{decimal_separator}{minors:0>width$}")
        } else {
            format!("{majors}{decimal_separator}{minors:0>width$} {currency_symbol}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MinorUnitExponent;

    #[test]
    fn should_preserve_currency_formatting_and_minor_units() {
        assert_eq!(
            Price::new(MonetaryAmount::from(542_u64), Currency::Eur).format_human_readable(),
            "5,42 €"
        );
        assert_eq!(
            Price::new(MonetaryAmount::from(500_u64), Currency::Jpy).format_human_readable(),
            "¥500"
        );
        assert_eq!(Currency::Jpy.minor_unit_exponent(), MinorUnitExponent(0));
        assert_eq!(Currency::Usd.minor_unit_exponent(), MinorUnitExponent(2));
    }

    #[test]
    fn should_reject_subtraction_below_zero() {
        assert_eq!(
            MonetaryAmount::from(1_u64) - MonetaryAmount::from(2_u64),
            Err(NegativeMonetaryAmountError)
        );
    }
}
