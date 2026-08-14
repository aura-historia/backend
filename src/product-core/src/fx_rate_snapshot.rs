use crate::fx_rate_id::FxRateId;
use common::{currency::domain::Currency, price::domain::Rate};
use std::collections::HashMap;
use strum::IntoEnumIterator;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FxRateSource {
    FxRatesApi,
}

impl FxRateSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FxRatesApi => "fxratesapi",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FxRateConversion {
    from_currency: Currency,
    to_currency: Currency,
    rate: Rate,
}

impl FxRateConversion {
    pub fn from_currency(&self) -> Currency {
        self.from_currency
    }

    pub fn to_currency(&self) -> Currency {
        self.to_currency
    }

    pub fn rate(&self) -> Rate {
        self.rate
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FxRateSnapshot {
    id: FxRateId,
    captured_at: OffsetDateTime,
    source: FxRateSource,
    conversions: Vec<FxRateConversion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CaptureFxRateSnapshotError {
    #[error("FX rate base currency must be EUR")]
    NonEurBaseCurrency,
    #[error("EUR must not be included in the quoted conversion currencies")]
    EurQuoteProvided,
    #[error("FX rate quote is duplicated for {0}")]
    DuplicateQuote(Currency),
    #[error("FX rate quote is missing for {0}")]
    MissingQuote(Currency),
    #[error("FX rate quote is invalid for {0}")]
    InvalidQuote(Currency),
}

impl FxRateSnapshot {
    pub fn capture_eur(
        id: FxRateId,
        captured_at: OffsetDateTime,
        source: FxRateSource,
        base: Currency,
        quotes: impl IntoIterator<Item = (Currency, Rate)>,
    ) -> Result<Self, CaptureFxRateSnapshotError> {
        if base != Currency::Eur {
            return Err(CaptureFxRateSnapshotError::NonEurBaseCurrency);
        }

        let mut rates = HashMap::new();
        for (currency, rate) in quotes {
            if currency == Currency::Eur {
                return Err(CaptureFxRateSnapshotError::EurQuoteProvided);
            }
            if rate == 0 {
                return Err(CaptureFxRateSnapshotError::InvalidQuote(currency));
            }
            if rates.insert(currency, rate).is_some() {
                return Err(CaptureFxRateSnapshotError::DuplicateQuote(currency));
            }
        }

        let mut conversions = Vec::new();
        for currency in Currency::iter().filter(|currency| *currency != Currency::Eur) {
            let rate = rates
                .remove(&currency)
                .ok_or(CaptureFxRateSnapshotError::MissingQuote(currency))?;
            conversions.push(FxRateConversion {
                from_currency: Currency::Eur,
                to_currency: currency,
                rate,
            });
        }

        Ok(Self {
            id,
            captured_at,
            source,
            conversions,
        })
    }

    pub fn id(&self) -> FxRateId {
        self.id
    }

    pub fn captured_at(&self) -> OffsetDateTime {
        self.captured_at
    }

    pub fn source(&self) -> FxRateSource {
        self.source
    }

    pub fn conversions(&self) -> &[FxRateConversion] {
        &self.conversions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete_quotes() -> Vec<(Currency, Rate)> {
        Currency::iter()
            .filter(|currency| *currency != Currency::Eur)
            .map(|currency| (currency, 1_250_000))
            .collect()
    }

    #[test]
    fn should_capture_complete_eur_snapshot_with_scaled_integer_rates() {
        let snapshot = FxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            complete_quotes(),
        );

        assert!(matches!(
            snapshot,
            Ok(snapshot)
                if snapshot.conversions().len() == Currency::iter().count() - 1
                    && snapshot.conversions().iter().all(|conversion|
                        conversion.from_currency() == Currency::Eur
                            && conversion.rate() == 1_250_000)
        ));
    }

    #[test]
    fn should_reject_incomplete_duplicate_and_zero_quotes() {
        let cases = [
            complete_quotes()
                .into_iter()
                .filter(|(currency, _)| *currency != Currency::Gbp)
                .collect(),
            {
                let mut quotes = complete_quotes();
                quotes.push((Currency::Gbp, 1_250_000));
                quotes
            },
            {
                let mut quotes = complete_quotes();
                quotes.retain(|(currency, _)| *currency != Currency::Usd);
                quotes.push((Currency::Usd, 0));
                quotes
            },
        ];

        for quotes in cases {
            let result = FxRateSnapshot::capture_eur(
                FxRateId::new(),
                OffsetDateTime::UNIX_EPOCH,
                FxRateSource::FxRatesApi,
                Currency::Eur,
                quotes,
            );
            assert!(result.is_err());
        }
    }

    #[test]
    fn should_reject_non_eur_base_and_eur_quote() {
        let non_eur_base = FxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Usd,
            complete_quotes(),
        );
        assert_eq!(
            Err(CaptureFxRateSnapshotError::NonEurBaseCurrency),
            non_eur_base
        );

        let mut quotes = complete_quotes();
        quotes.push((Currency::Eur, 1_000_000));
        let eur_quote = FxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            quotes,
        );
        assert_eq!(Err(CaptureFxRateSnapshotError::EurQuoteProvided), eur_quote);
    }
}
