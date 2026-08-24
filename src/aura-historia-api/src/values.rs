use geo::core::distance::{Distance, DistanceUnit, GeoDistanceQuery};
use localization::{Language, Localized};
use money::{Currency, MonetaryAmount, Price};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct DistanceData {
    pub(crate) amount: f64,
    #[serde(with = "crate::wire::distance_unit")]
    pub(crate) unit: DistanceUnit,
}

impl From<DistanceData> for Distance {
    fn from(value: DistanceData) -> Self {
        Self {
            amount: value.amount,
            unit: value.unit,
        }
    }
}

impl From<Distance> for DistanceData {
    fn from(value: Distance) -> Self {
        Self {
            amount: value.amount,
            unit: value.unit,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalizedTextData {
    pub(crate) text: String,
    #[serde(with = "crate::wire::language")]
    pub(crate) language: Language,
}

impl LocalizedTextData {
    pub(crate) fn into_localized<T: From<String>>(self) -> Localized<Language, T> {
        Localized::new(self.language, self.text.into())
    }
}

impl<T: Into<String>> From<Localized<Language, T>> for LocalizedTextData {
    fn from(value: Localized<Language, T>) -> Self {
        Self {
            text: value.payload.into(),
            language: value.localization,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PriceData {
    #[serde(with = "crate::wire::currency")]
    pub(crate) currency: Currency,
    pub(crate) amount: u64,
}

impl From<PriceData> for Price {
    fn from(value: PriceData) -> Self {
        Self::new(MonetaryAmount::from(value.amount), value.currency)
    }
}

impl From<Price> for PriceData {
    fn from(value: Price) -> Self {
        Self {
            currency: value.currency,
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
        assert_eq!(
            serde_json::json!({ "currency": "EUR", "amount": 1 }),
            serde_json::to_value(PriceData {
                currency: Currency::Eur,
                amount: 1,
            })?
        );
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
                    unit: DistanceUnit::Kilometers,
                },
            })?
        );
        assert_eq!(
            Currency::Usd,
            serde_json::from_str::<PriceData>(r#"{"currency":"USD","amount":1}"#)?.currency
        );
        assert_eq!(
            Language::De,
            serde_json::from_str::<LocalizedTextData>(r#"{"text":"x","language":"de-CH"}"#)?
                .language
        );
        assert_eq!(
            serde_json::json!({ "text": "Cabinet", "language": "en" }),
            serde_json::to_value(LocalizedTextData {
                text: "Cabinet".to_owned(),
                language: Language::En,
            })?
        );
        Ok(())
    }

    #[test]
    fn should_map_prices_and_localized_text_at_the_api_boundary() {
        let price = PriceData {
            currency: Currency::Eur,
            amount: 1_200,
        };
        assert_eq!(
            Price::new(MonetaryAmount::from(1_200_u64), Currency::Eur),
            price.into()
        );

        let text = LocalizedTextData {
            text: "Cabinet".to_owned(),
            language: Language::En,
        };
        let localized: Localized<Language, String> = text.into_localized();
        assert_eq!(
            Localized::new(Language::En, "Cabinet".to_owned()),
            localized
        );
    }
}
