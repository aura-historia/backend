use crate::currency::domain::{Currency, HasMinorUnitExponent, MinorUnitExponent};
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CurrencyData {
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

impl HasMinorUnitExponent for CurrencyData {
    fn minor_unit_exponent(&self) -> MinorUnitExponent {
        Currency::from(*self).minor_unit_exponent()
    }
}

impl From<Currency> for CurrencyData {
    fn from(domain: Currency) -> Self {
        match domain {
            Currency::Eur => CurrencyData::Eur,
            Currency::Gbp => CurrencyData::Gbp,
            Currency::Usd => CurrencyData::Usd,
            Currency::Aud => CurrencyData::Aud,
            Currency::Cad => CurrencyData::Cad,
            Currency::Nzd => CurrencyData::Nzd,
            Currency::Cny => CurrencyData::Cny,
            Currency::Brl => CurrencyData::Brl,
            Currency::Pln => CurrencyData::Pln,
            Currency::Try => CurrencyData::Try,
            Currency::Jpy => CurrencyData::Jpy,
            Currency::Czk => CurrencyData::Czk,
            Currency::Rub => CurrencyData::Rub,
            Currency::Aed => CurrencyData::Aed,
            Currency::Sar => CurrencyData::Sar,
            Currency::Hkd => CurrencyData::Hkd,
            Currency::Sgd => CurrencyData::Sgd,
            Currency::Chf => CurrencyData::Chf,
        }
    }
}

#[cfg(feature = "api")]
pub mod api {
    use crate::{
        api::{error::ApiError, error_code::BAD_QUERY_PARAMETER_VALUE},
        currency::data::CurrencyData,
    };
    use aws_lambda_events::query_map::QueryMap;

    pub fn extract_currency_query(query: &QueryMap) -> Result<CurrencyData, ApiError> {
        let currency = query
            .first("currency")
            .filter(|str| !str.is_empty())
            .map(|currency| serde_json::from_str::<CurrencyData>(&format!(r#""{currency}""#)))
            .map(|currency_res| {
                currency_res.map_err(|err| {
                    let msg = err.to_string();
                    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE, Box::new(err))
                        .with_query_field("currency")
                        .with_detail(msg)
                })
            })
            .transpose()?
            .unwrap_or_default();

        Ok(currency)
    }

    #[cfg(test)]
    mod tests {
        use rstest;

        use crate::api::{
            error::{ApiErrorSource, ApiErrorSourceType},
            error_code::BAD_QUERY_PARAMETER_VALUE,
        };
        use crate::currency::data::CurrencyData;
        use crate::currency::data::api::extract_currency_query;
        use aws_lambda_events::query_map::QueryMap;
        use std::collections::HashMap;

        #[rstest::rstest]
        #[case::eur("EUR", CurrencyData::Eur)]
        #[case::gbp("GBP", CurrencyData::Gbp)]
        #[case::usd("USD", CurrencyData::Usd)]
        #[case::aud("AUD", CurrencyData::Aud)]
        #[case::cad("CAD", CurrencyData::Cad)]
        #[case::nzd("NZD", CurrencyData::Nzd)]
        #[case::cny("CNY", CurrencyData::Cny)]
        #[case::brl("BRL", CurrencyData::Brl)]
        #[case::pln("PLN", CurrencyData::Pln)]
        #[case::try_("TRY", CurrencyData::Try)]
        #[case::jpy("JPY", CurrencyData::Jpy)]
        #[case::czk("CZK", CurrencyData::Czk)]
        #[case::rub("RUB", CurrencyData::Rub)]
        #[case::aed("AED", CurrencyData::Aed)]
        #[case::sar("SAR", CurrencyData::Sar)]
        #[case::hkd("HKD", CurrencyData::Hkd)]
        #[case::sgd("SGD", CurrencyData::Sgd)]
        #[case::chf("CHF", CurrencyData::Chf)]
        #[trace]
        fn should_extract_currency(#[case] query_value: String, #[case] expected: CurrencyData) {
            let query = QueryMap::from(HashMap::from_iter([("currency".to_string(), query_value)]));

            let actual = extract_currency_query(&query).unwrap();

            assert_eq!(expected, actual);
        }

        #[rstest::rstest]
        #[case("invalid-currency")]
        #[case("boop")]
        #[case("euronen")]
        #[case("dollers")]
        #[case("moneten")]
        #[case("knete")]
        #[case("knöpfe")]
        #[trace]
        fn should_400_when_currency_query_param_is_invalid(#[case] query_value: String) {
            let query = QueryMap::from(HashMap::from_iter([("currency".to_string(), query_value)]));

            let actual = extract_currency_query(&query).unwrap_err();

            assert_eq!(400, actual.status);
            assert_eq!(BAD_QUERY_PARAMETER_VALUE, actual.error);
            assert_eq!(
                Some(ApiErrorSource {
                    field: "currency",
                    source_type: ApiErrorSourceType::Query,
                }),
                actual.source
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CurrencyData;
    use rstest::rstest;

    #[rstest]
    #[case(CurrencyData::Eur, "\"EUR\"")]
    #[case(CurrencyData::Gbp, "\"GBP\"")]
    #[case(CurrencyData::Usd, "\"USD\"")]
    #[case(CurrencyData::Aud, "\"AUD\"")]
    #[case(CurrencyData::Cad, "\"CAD\"")]
    #[case(CurrencyData::Nzd, "\"NZD\"")]
    #[case(CurrencyData::Cny, "\"CNY\"")]
    #[case(CurrencyData::Brl, "\"BRL\"")]
    #[case(CurrencyData::Pln, "\"PLN\"")]
    #[case(CurrencyData::Try, "\"TRY\"")]
    #[case(CurrencyData::Jpy, "\"JPY\"")]
    #[case(CurrencyData::Czk, "\"CZK\"")]
    #[case(CurrencyData::Rub, "\"RUB\"")]
    #[case(CurrencyData::Aed, "\"AED\"")]
    #[case(CurrencyData::Sar, "\"SAR\"")]
    #[case(CurrencyData::Hkd, "\"HKD\"")]
    #[case(CurrencyData::Sgd, "\"SGD\"")]
    #[case(CurrencyData::Chf, "\"CHF\"")]
    #[trace]
    fn should_serialize_currency_according_to_iso_4217(
        #[case] currency: CurrencyData,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&currency).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"EUR\"", CurrencyData::Eur)]
    #[case("\"GBP\"", CurrencyData::Gbp)]
    #[case("\"USD\"", CurrencyData::Usd)]
    #[case("\"AUD\"", CurrencyData::Aud)]
    #[case("\"CAD\"", CurrencyData::Cad)]
    #[case("\"NZD\"", CurrencyData::Nzd)]
    #[case("\"CNY\"", CurrencyData::Cny)]
    #[case("\"BRL\"", CurrencyData::Brl)]
    #[case("\"PLN\"", CurrencyData::Pln)]
    #[case("\"TRY\"", CurrencyData::Try)]
    #[case("\"JPY\"", CurrencyData::Jpy)]
    #[case("\"CZK\"", CurrencyData::Czk)]
    #[case("\"RUB\"", CurrencyData::Rub)]
    #[case("\"AED\"", CurrencyData::Aed)]
    #[case("\"SAR\"", CurrencyData::Sar)]
    #[case("\"HKD\"", CurrencyData::Hkd)]
    #[case("\"SGD\"", CurrencyData::Sgd)]
    #[case("\"CHF\"", CurrencyData::Chf)]
    #[trace]
    fn should_deserialize_currency_according_to_iso_4217(
        #[case] currency: &str,
        #[case] expected: CurrencyData,
    ) {
        let actual = serde_json::from_str::<CurrencyData>(currency).unwrap();
        assert_eq!(actual, expected);
    }
}
