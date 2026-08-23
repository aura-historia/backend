use geo::core::distance::{Distance, DistanceUnit, GeoDistanceQuery};
use localization::{Language, Localized};
use money::{Currency, MonetaryAmount, Price};
use serde::{Deserialize, Serialize};
use user_core::measurement_unit::MeasurementUnit;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum CurrencyData {
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

impl From<CurrencyData> for Currency {
    fn from(value: CurrencyData) -> Self {
        match value {
            CurrencyData::Eur => Self::Eur,
            CurrencyData::Gbp => Self::Gbp,
            CurrencyData::Usd => Self::Usd,
            CurrencyData::Aud => Self::Aud,
            CurrencyData::Cad => Self::Cad,
            CurrencyData::Nzd => Self::Nzd,
            CurrencyData::Cny => Self::Cny,
            CurrencyData::Brl => Self::Brl,
            CurrencyData::Pln => Self::Pln,
            CurrencyData::Try => Self::Try,
            CurrencyData::Jpy => Self::Jpy,
            CurrencyData::Czk => Self::Czk,
            CurrencyData::Rub => Self::Rub,
            CurrencyData::Aed => Self::Aed,
            CurrencyData::Sar => Self::Sar,
            CurrencyData::Hkd => Self::Hkd,
            CurrencyData::Sgd => Self::Sgd,
            CurrencyData::Chf => Self::Chf,
        }
    }
}

impl From<Currency> for CurrencyData {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum LanguageData {
    #[serde(
        alias = "de-DE",
        alias = "de-AT",
        alias = "de-CH",
        alias = "de-LU",
        alias = "de-LI"
    )]
    De,
    #[serde(
        alias = "en-US",
        alias = "en-GB",
        alias = "en-AU",
        alias = "en-CA",
        alias = "en-NZ",
        alias = "en_IE"
    )]
    #[default]
    En,
    #[serde(
        alias = "fr-FR",
        alias = "fr-CA",
        alias = "fr-BE",
        alias = "fr-CH",
        alias = "fr-LU"
    )]
    Fr,
    #[serde(
        alias = "es-ES",
        alias = "es-MX",
        alias = "es-AR",
        alias = "es-CO",
        alias = "es-CL",
        alias = "es-PE",
        alias = "es-VE"
    )]
    Es,
    #[serde(alias = "it-IT", alias = "it-CH")]
    It,
    #[serde(alias = "zh-CN", alias = "zh-Hans")]
    Zh,
    #[serde(alias = "pt-PT", alias = "pt-BR")]
    Pt,
    #[serde(alias = "pl-PL")]
    Pl,
    #[serde(alias = "tr-TR")]
    Tr,
    #[serde(alias = "nl-NL", alias = "nl-BE")]
    Nl,
    #[serde(alias = "cs-CZ")]
    Cs,
    #[serde(alias = "ja-JP")]
    Ja,
    #[serde(alias = "ru-RU")]
    Ru,
    #[serde(alias = "ar-SA", alias = "ar-EG", alias = "ar-AE")]
    Ar,
}

impl From<LanguageData> for Language {
    fn from(value: LanguageData) -> Self {
        match value {
            LanguageData::De => Self::De,
            LanguageData::En => Self::En,
            LanguageData::Fr => Self::Fr,
            LanguageData::Es => Self::Es,
            LanguageData::It => Self::It,
            LanguageData::Zh => Self::Zh,
            LanguageData::Pt => Self::Pt,
            LanguageData::Pl => Self::Pl,
            LanguageData::Tr => Self::Tr,
            LanguageData::Nl => Self::Nl,
            LanguageData::Cs => Self::Cs,
            LanguageData::Ja => Self::Ja,
            LanguageData::Ru => Self::Ru,
            LanguageData::Ar => Self::Ar,
        }
    }
}

impl From<Language> for LanguageData {
    fn from(value: Language) -> Self {
        match value {
            Language::De => Self::De,
            Language::En => Self::En,
            Language::Fr => Self::Fr,
            Language::Es => Self::Es,
            Language::It => Self::It,
            Language::Zh => Self::Zh,
            Language::Pt => Self::Pt,
            Language::Pl => Self::Pl,
            Language::Tr => Self::Tr,
            Language::Nl => Self::Nl,
            Language::Cs => Self::Cs,
            Language::Ja => Self::Ja,
            Language::Ru => Self::Ru,
            Language::Ar => Self::Ar,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct DistanceData {
    pub(crate) amount: f64,
    pub(crate) unit: DistanceUnitData,
}

impl From<DistanceData> for Distance {
    fn from(value: DistanceData) -> Self {
        Self {
            amount: value.amount,
            unit: value.unit.into(),
        }
    }
}

impl From<Distance> for DistanceData {
    fn from(value: Distance) -> Self {
        Self {
            amount: value.amount,
            unit: value.unit.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct GeoDistanceQueryData {
    pub(crate) lat: f64,
    pub(crate) lon: f64,
    pub(crate) distance: DistanceData,
}

impl From<GeoDistanceQueryData> for GeoDistanceQuery {
    fn from(value: GeoDistanceQueryData) -> Self {
        Self {
            lat: value.lat,
            lon: value.lon,
            distance: value.distance.into(),
        }
    }
}

impl From<GeoDistanceQuery> for GeoDistanceQueryData {
    fn from(value: GeoDistanceQuery) -> Self {
        Self {
            lat: value.lat,
            lon: value.lon,
            distance: value.distance.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum DistanceUnitData {
    Miles,
    Yards,
    Feet,
    Inches,
    Kilometers,
    Meters,
    Centimeters,
    Millimeters,
    NauticalMiles,
}

impl From<DistanceUnitData> for DistanceUnit {
    fn from(value: DistanceUnitData) -> Self {
        match value {
            DistanceUnitData::Miles => Self::Miles,
            DistanceUnitData::Yards => Self::Yards,
            DistanceUnitData::Feet => Self::Feet,
            DistanceUnitData::Inches => Self::Inches,
            DistanceUnitData::Kilometers => Self::Kilometers,
            DistanceUnitData::Meters => Self::Meters,
            DistanceUnitData::Centimeters => Self::Centimeters,
            DistanceUnitData::Millimeters => Self::Millimeters,
            DistanceUnitData::NauticalMiles => Self::NauticalMiles,
        }
    }
}

impl From<DistanceUnit> for DistanceUnitData {
    fn from(value: DistanceUnit) -> Self {
        match value {
            DistanceUnit::Miles => Self::Miles,
            DistanceUnit::Yards => Self::Yards,
            DistanceUnit::Feet => Self::Feet,
            DistanceUnit::Inches => Self::Inches,
            DistanceUnit::Kilometers => Self::Kilometers,
            DistanceUnit::Meters => Self::Meters,
            DistanceUnit::Centimeters => Self::Centimeters,
            DistanceUnit::Millimeters => Self::Millimeters,
            DistanceUnit::NauticalMiles => Self::NauticalMiles,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum MeasurementUnitData {
    Metric,
    Imperial,
}

impl From<MeasurementUnitData> for MeasurementUnit {
    fn from(value: MeasurementUnitData) -> Self {
        match value {
            MeasurementUnitData::Metric => Self::Metric,
            MeasurementUnitData::Imperial => Self::Imperial,
        }
    }
}

impl From<MeasurementUnit> for MeasurementUnitData {
    fn from(value: MeasurementUnit) -> Self {
        match value {
            MeasurementUnit::Metric => Self::Metric,
            MeasurementUnit::Imperial => Self::Imperial,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalizedTextData {
    pub(crate) text: String,
    pub(crate) language: LanguageData,
}

impl LocalizedTextData {
    pub(crate) fn into_localized<T: From<String>>(self) -> Localized<Language, T> {
        Localized::new(self.language.into(), self.text.into())
    }
}

impl<T: Into<String>> From<Localized<Language, T>> for LocalizedTextData {
    fn from(value: Localized<Language, T>) -> Self {
        Self {
            text: value.payload.into(),
            language: value.localization.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PriceData {
    pub(crate) currency: CurrencyData,
    pub(crate) amount: u64,
}

impl From<PriceData> for Price {
    fn from(value: PriceData) -> Self {
        Self::new(MonetaryAmount::from(value.amount), value.currency.into())
    }
}

impl From<Price> for PriceData {
    fn from(value: Price) -> Self {
        Self {
            currency: value.currency.into(),
            amount: value.monetary_amount.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_preserve_currency_and_localized_text_json_contract()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!("\"EUR\"", serde_json::to_string(&CurrencyData::Eur)?);
        assert_eq!(
            serde_json::json!({
                "lat": 52.52,
                "lon": 13.405,
                "distance": { "amount": 50.0, "unit": "KILOMETERS" }
            }),
            serde_json::to_value(GeoDistanceQueryData {
                lat: 52.52,
                lon: 13.405,
                distance: DistanceData {
                    amount: 50.0,
                    unit: DistanceUnitData::Kilometers,
                },
            })?
        );
        assert_eq!(CurrencyData::Usd, serde_json::from_str("\"USD\"")?);
        assert_eq!(LanguageData::De, serde_json::from_str("\"de-CH\"")?);
        assert_eq!("\"en\"", serde_json::to_string(&LanguageData::En)?);
        assert_eq!(
            serde_json::json!({ "text": "Cabinet", "language": "en" }),
            serde_json::to_value(LocalizedTextData {
                text: "Cabinet".to_owned(),
                language: LanguageData::En,
            })?
        );
        Ok(())
    }

    #[test]
    fn should_map_prices_and_localized_text_at_the_api_boundary() {
        let price = PriceData {
            currency: CurrencyData::Eur,
            amount: 1_200,
        };
        assert_eq!(
            Price::new(MonetaryAmount::from(1_200_u64), Currency::Eur),
            price.into()
        );

        let text = LocalizedTextData {
            text: "Cabinet".to_owned(),
            language: LanguageData::En,
        };
        let localized: Localized<Language, String> = text.into_localized();
        assert_eq!(
            Localized::new(Language::En, "Cabinet".to_owned()),
            localized
        );
    }
}
