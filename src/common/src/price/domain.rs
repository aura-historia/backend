use crate::currency::domain::{Currency, HasMinorUnitExponent};
use crate::price::data::PriceData;
use crate::price::record::PriceRecord;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::{Add, Deref, Sub};
use strum::{EnumCount, IntoEnumIterator};

pub type Rate = u64;
pub const FX_RATE_SCALE: Rate = 1_000_000;

pub trait FxRate {
    fn exchange(
        &self,
        from_currency: Currency,
        to_currency: Currency,
        from_amount: MonetaryAmount,
    ) -> Result<MonetaryAmount, MonetaryAmountOverflowError>;

    fn exchange_all(
        &self,
        from_currency: Currency,
        from_amount: MonetaryAmount,
    ) -> Result<HashMap<Currency, MonetaryAmount>, MonetaryAmountOverflowError> {
        let mut exchanged = HashMap::with_capacity(Currency::COUNT);
        for currency in Currency::iter() {
            exchanged.insert(
                currency,
                self.exchange(from_currency, currency, from_amount)?,
            );
        }
        Ok(exchanged)
    }
}

/// as of 2025-07-15
#[derive(Default)]
pub struct FixedFxRate();

impl FixedFxRate {
    fn eur_base_rate(currency: Currency) -> Rate {
        match currency {
            Currency::Eur => 1_000_000,
            Currency::Usd => 1_117_000,
            Currency::Gbp => 843_000,
            Currency::Aud => 1_748_000,
            Currency::Cad => 1_557_000,
            Currency::Nzd => 1_900_000,
            Currency::Cny => 8_150_000,
            Currency::Brl => 6_100_000,
            Currency::Pln => 4_270_000,
            Currency::Try => 40_500_000,
            Currency::Jpy => 163_000_000,
            Currency::Czk => 25_100_000,
            Currency::Rub => 100_000_000,
            Currency::Aed => 4_103_000,
            Currency::Sar => 4_190_000,
            Currency::Hkd => 8_711_000,
            Currency::Sgd => 1_488_000,
            Currency::Chf => 944_000,
        }
    }

    pub fn get_rate(&self, from: Currency, to: Currency) -> Rate {
        if from == to {
            return FX_RATE_SCALE;
        }
        let eur_from = Self::eur_base_rate(from);
        let eur_to = Self::eur_base_rate(to);
        let numerator = u128::from(eur_to) * u128::from(FX_RATE_SCALE);
        let rounded = (numerator + u128::from(eur_from / 2)) / u128::from(eur_from);
        rounded as Rate
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, thiserror::Error)]
#[error("Monetary amount overflowed during an internal operation.")]
pub struct MonetaryAmountOverflowError;

impl FxRate for FixedFxRate {
    fn exchange(
        &self,
        from_currency: Currency,
        to_currency: Currency,
        from_amount: MonetaryAmount,
    ) -> Result<MonetaryAmount, MonetaryAmountOverflowError> {
        let rate = self.get_rate(from_currency, to_currency);

        // Half-Up Rounding
        let numerator = from_amount
            .0
            .checked_mul(rate)
            .ok_or(MonetaryAmountOverflowError)?;
        let half = FX_RATE_SCALE / 2;
        let converted = (numerator + half) / FX_RATE_SCALE;

        Ok(MonetaryAmount(converted))
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
    type Output = MonetaryAmount;

    fn add(self, rhs: Self) -> Self::Output {
        MonetaryAmount(self.0 + rhs.0)
    }
}

impl Sub for MonetaryAmount {
    type Output = Result<MonetaryAmount, NegativeMonetaryAmountError>;

    fn sub(self, rhs: Self) -> Self::Output {
        if self.0 < rhs.0 {
            Err(NegativeMonetaryAmountError)
        } else {
            Ok(MonetaryAmount(self.0 - rhs.0))
        }
    }
}

impl From<u8> for MonetaryAmount {
    fn from(amount: u8) -> Self {
        MonetaryAmount(amount as u64)
    }
}

impl From<u16> for MonetaryAmount {
    fn from(amount: u16) -> Self {
        MonetaryAmount(amount as u64)
    }
}

impl From<u32> for MonetaryAmount {
    fn from(amount: u32) -> Self {
        MonetaryAmount(amount as u64)
    }
}

impl From<u64> for MonetaryAmount {
    fn from(amount: u64) -> Self {
        MonetaryAmount(amount)
    }
}

impl From<MonetaryAmount> for u64 {
    fn from(price: MonetaryAmount) -> Self {
        price.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub monetary_amount: MonetaryAmount,
    pub currency: Currency,
}

impl Price {
    pub fn new(monetary_amount: MonetaryAmount, currency: Currency) -> Self {
        Price {
            monetary_amount,
            currency,
        }
    }

    pub fn into_exchanged(
        self,
        fx_rate: &impl FxRate,
        currency: Currency,
    ) -> Result<Price, MonetaryAmountOverflowError> {
        let exchanged = Price {
            monetary_amount: fx_rate.exchange(self.currency, currency, self.monetary_amount)?,
            currency,
        };
        Ok(exchanged)
    }

    pub fn exchanged(
        &mut self,
        fx_rate: &impl FxRate,
        currency: Currency,
    ) -> Result<(), MonetaryAmountOverflowError> {
        self.monetary_amount =
            fx_rate.exchange(self.currency, self.currency, self.monetary_amount)?;
        self.currency = currency;
        Ok(())
    }

    pub fn format_human_readable(&self) -> String {
        let exponent = self.currency.minor_unit_exponent().0 as u32;
        let currency_symbol = self.currency.currency_symbol();

        if exponent == 0 {
            let majors = self.monetary_amount.0;
            if self.currency.is_leading_sign() {
                format!("{currency_symbol}{majors}")
            } else {
                format!("{majors} {currency_symbol}")
            }
        } else {
            let divisor = 10u64.pow(exponent);
            let majors = self.monetary_amount.0 / divisor;
            let minors = self.monetary_amount.0 % divisor;
            let width = exponent as usize;
            let decimal_separator = self.currency.decimal_separator();

            if self.currency.is_leading_sign() {
                format!("{currency_symbol}{majors}{decimal_separator}{minors:0>width$}",)
            } else {
                format!("{majors}{decimal_separator}{minors:0>width$} {currency_symbol}",)
            }
        }
    }
}

impl From<PriceData> for Price {
    fn from(data: PriceData) -> Self {
        Price {
            monetary_amount: data.amount.into(),
            currency: data.currency.into(),
        }
    }
}

impl From<PriceRecord> for Price {
    fn from(record: PriceRecord) -> Self {
        Price {
            monetary_amount: record.amount.into(),
            currency: record.currency.into(),
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use crate::price::domain::Price;
    use fake::{Dummy, Fake, Faker, RngExt};
    use std::ops::Range;

    impl Dummy<Faker> for Price {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            Price {
                monetary_amount: rng
                    .random_range(Range {
                        start: 100u64,
                        end: 9999999u64,
                    })
                    .into(),
                currency: config.fake_with_rng(rng),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest;

    use crate::currency::domain::Currency;
    use crate::price::domain::{FxRate, MonetaryAmount, MonetaryAmountOverflowError, Price};

    struct DummyFxRate;
    impl FxRate for DummyFxRate {
        fn exchange(
            &self,
            _: Currency,
            _: Currency,
            from_amount: MonetaryAmount,
        ) -> Result<MonetaryAmount, MonetaryAmountOverflowError> {
            Ok(MonetaryAmount(from_amount.0 * 2))
        }
    }

    #[test]
    fn should_into_exchanged() {
        let price = Price {
            monetary_amount: MonetaryAmount(500),
            currency: Currency::Eur,
        };

        let exchanged = price.into_exchanged(&DummyFxRate, Currency::Gbp);

        assert_eq!(1000, exchanged.unwrap().monetary_amount.0);
    }

    #[test]
    fn should_exchange() {
        let mut price = Price {
            monetary_amount: MonetaryAmount(500),
            currency: Currency::Eur,
        };

        let res = price.exchanged(&DummyFxRate, Currency::Gbp);

        assert!(res.is_ok());
        assert_eq!(1000, price.monetary_amount.0);
    }

    #[rstest::rstest]
    #[case(Price::new(MonetaryAmount(500), Currency::Eur), "5,00 €")]
    #[case(Price::new(MonetaryAmount(542), Currency::Eur), "5,42 €")]
    #[case(Price::new(MonetaryAmount(123456), Currency::Eur), "1234,56 €")]
    #[case(Price::new(MonetaryAmount(500), Currency::Gbp), "£5.00")]
    #[case(Price::new(MonetaryAmount(542), Currency::Gbp), "£5.42")]
    #[case(Price::new(MonetaryAmount(123456), Currency::Gbp), "£1234.56")]
    #[case(Price::new(MonetaryAmount(500), Currency::Usd), "$5.00")]
    #[case(Price::new(MonetaryAmount(542), Currency::Usd), "$5.42")]
    #[case(Price::new(MonetaryAmount(123456), Currency::Usd), "$1234.56")]
    #[case(Price::new(MonetaryAmount(500), Currency::Aud), "A$5.00")]
    #[case(Price::new(MonetaryAmount(542), Currency::Aud), "A$5.42")]
    #[case(Price::new(MonetaryAmount(123456), Currency::Aud), "A$1234.56")]
    #[case(Price::new(MonetaryAmount(500), Currency::Cad), "C$5.00")]
    #[case(Price::new(MonetaryAmount(542), Currency::Cad), "C$5.42")]
    #[case(Price::new(MonetaryAmount(123456), Currency::Cad), "C$1234.56")]
    #[case(Price::new(MonetaryAmount(500), Currency::Nzd), "NZ$5.00")]
    #[case(Price::new(MonetaryAmount(542), Currency::Nzd), "NZ$5.42")]
    #[case(Price::new(MonetaryAmount(123456), Currency::Nzd), "NZ$1234.56")]
    #[case(Price::new(MonetaryAmount(500), Currency::Cny), "CN¥5.00")]
    #[case(Price::new(MonetaryAmount(123456), Currency::Cny), "CN¥1234.56")]
    #[case(Price::new(MonetaryAmount(500), Currency::Brl), "R$5,00")]
    #[case(Price::new(MonetaryAmount(123456), Currency::Brl), "R$1234,56")]
    #[case(Price::new(MonetaryAmount(500), Currency::Pln), "5,00 zł")]
    #[case(Price::new(MonetaryAmount(123456), Currency::Pln), "1234,56 zł")]
    #[case(Price::new(MonetaryAmount(500), Currency::Try), "5,00 ₺")]
    #[case(Price::new(MonetaryAmount(123456), Currency::Try), "1234,56 ₺")]
    #[case(Price::new(MonetaryAmount(1234), Currency::Jpy), "¥1234")]
    #[case(Price::new(MonetaryAmount(500), Currency::Jpy), "¥500")]
    #[case(Price::new(MonetaryAmount(0), Currency::Jpy), "¥0")]
    #[case(Price::new(MonetaryAmount(500), Currency::Czk), "5,00 Kč")]
    #[case(Price::new(MonetaryAmount(123456), Currency::Czk), "1234,56 Kč")]
    #[case(Price::new(MonetaryAmount(500), Currency::Rub), "5,00 ₽")]
    #[case(Price::new(MonetaryAmount(123456), Currency::Rub), "1234,56 ₽")]
    #[case(Price::new(MonetaryAmount(500), Currency::Aed), "5.00 د.إ")]
    #[case(Price::new(MonetaryAmount(500), Currency::Sar), "5.00 ﷼")]
    #[case(Price::new(MonetaryAmount(500), Currency::Hkd), "HK$5.00")]
    #[case(Price::new(MonetaryAmount(123456), Currency::Hkd), "HK$1234.56")]
    #[case(Price::new(MonetaryAmount(500), Currency::Sgd), "S$5.00")]
    #[case(Price::new(MonetaryAmount(123456), Currency::Sgd), "S$1234.56")]
    #[case(Price::new(MonetaryAmount(500), Currency::Chf), "CHF5.00")]
    #[case(Price::new(MonetaryAmount(123456), Currency::Chf), "CHF1234.56")]
    #[trace]
    fn should_format_price_human_readable(#[case] price: Price, #[case] expected: &str) {
        let actual = price.format_human_readable();

        assert_eq!(expected, &actual);
    }
}
