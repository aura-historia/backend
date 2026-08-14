use common::{
    currency::{data::CurrencyData, domain::Currency},
    error::boxed::{box_error, static_error},
    price::domain::Rate,
};
use product_service::ports::{
    FxRateQuote, FxRateQuoteProvider, FxRateQuoteProviderError, FxRateQuoteSet,
};
use serde::Deserialize;
use std::collections::HashMap;
use strum::IntoEnumIterator;

const FX_RATES_API_URL: &str = "https://api.fxratesapi.com/latest";
const FX_RATE_DECIMAL_PLACES: i64 = 6;

#[derive(Debug, Clone)]
pub struct FxRatesApiQuoteProvider {
    client: reqwest::Client,
    token: String,
}

#[derive(Debug, Deserialize)]
struct FxRatesApiResponse {
    success: bool,
    base: CurrencyData,
    rates: HashMap<CurrencyData, serde_json::Number>,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("FX rate must be a positive decimal number within the supported range")]
struct InvalidFxRate;

impl FxRatesApiQuoteProvider {
    pub fn new(client: reqwest::Client, token: impl Into<String>) -> Self {
        Self {
            client,
            token: token.into(),
        }
    }
}

#[async_trait::async_trait]
impl FxRateQuoteProvider for FxRatesApiQuoteProvider {
    async fn fetch_eur_quotes(&self) -> Result<FxRateQuoteSet, FxRateQuoteProviderError> {
        let currencies = Currency::iter()
            .filter(|currency| *currency != Currency::Eur)
            .map(|currency| currency.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let response = self
            .client
            .get(FX_RATES_API_URL)
            .query(&[("base", "EUR"), ("currencies", currencies.as_str())])
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|source| FxRateQuoteProviderError::RequestFailed {
                source: box_error(source),
            })?
            .error_for_status()
            .map_err(|source| FxRateQuoteProviderError::RequestFailed {
                source: box_error(source),
            })?
            .json::<FxRatesApiResponse>()
            .await
            .map_err(|source| FxRateQuoteProviderError::InvalidResponse {
                source: box_error(source),
            })?;
        if !response.success {
            return Err(FxRateQuoteProviderError::InvalidResponse {
                source: static_error("FxRatesApi returned success=false"),
            });
        }

        let quotes = response
            .rates
            .into_iter()
            .map(|(currency, rate)| {
                decimal_rate_to_scaled_integer(&rate).map(|rate| FxRateQuote {
                    currency: currency.into(),
                    rate,
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| FxRateQuoteProviderError::InvalidResponse {
                source: box_error(source),
            })?;

        Ok(FxRateQuoteSet {
            base: response.base.into(),
            quotes,
        })
    }
}

fn decimal_rate_to_scaled_integer(value: &serde_json::Number) -> Result<Rate, InvalidFxRate> {
    let value = value.to_string();
    let (significand, exponent) = match value.split_once(['e', 'E']) {
        Some((significand, exponent)) => (
            significand,
            exponent.parse::<i32>().map_err(|_| InvalidFxRate)?,
        ),
        None => (value.as_str(), 0),
    };
    let (whole, fractional) = match significand.split_once('.') {
        Some((whole, fractional)) => (whole, fractional),
        None => (significand, ""),
    };
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fractional.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(InvalidFxRate);
    }

    let significant = format!("{whole}{fractional}")
        .parse::<Rate>()
        .map_err(|_| InvalidFxRate)?;
    let shift = i64::from(exponent) - i64::try_from(fractional.len()).map_err(|_| InvalidFxRate)?
        + FX_RATE_DECIMAL_PLACES;
    let scaled = if shift >= 0 {
        significant
            .checked_mul(power_of_ten(
                u32::try_from(shift).map_err(|_| InvalidFxRate)?,
            )?)
            .ok_or(InvalidFxRate)?
    } else {
        let divisor = power_of_ten(u32::try_from(-shift).map_err(|_| InvalidFxRate)?)?;
        let quotient = significant / divisor;
        if significant % divisor == 0 {
            quotient
        } else {
            quotient.checked_add(1).ok_or(InvalidFxRate)?
        }
    };

    if scaled == 0 {
        return Err(InvalidFxRate);
    }
    Ok(scaled)
}

fn power_of_ten(exponent: u32) -> Result<Rate, InvalidFxRate> {
    (0..exponent).try_fold(1_u64, |value, _| value.checked_mul(10).ok_or(InvalidFxRate))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(value: &str) -> serde_json::Number {
        match serde_json::from_str(value) {
            Ok(value) => value,
            Err(error) => panic!("test number must be valid JSON: {error}"),
        }
    }

    #[test]
    fn should_convert_decimal_rates_to_scaled_unsigned_integers() {
        for (input, expected) in [
            ("1", 1_000_000),
            ("1.25", 1_250_000),
            ("0.0000001", 1),
            ("1.0000001", 1_000_001),
            ("1.0000000000000001", 1_000_001),
            ("1.0000000", 1_000_000),
        ] {
            assert_eq!(Ok(expected), decimal_rate_to_scaled_integer(&number(input)));
        }
    }

    #[test]
    fn should_reject_zero_and_negative_rates() {
        for input in ["0", "-1"] {
            assert!(decimal_rate_to_scaled_integer(&number(input)).is_err());
        }
    }
}
