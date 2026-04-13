use crate::currency::domain::Currency;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CurrencyRecord {
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

impl From<Currency> for CurrencyRecord {
    fn from(domain: Currency) -> Self {
        match domain {
            Currency::Eur => CurrencyRecord::Eur,
            Currency::Gbp => CurrencyRecord::Gbp,
            Currency::Usd => CurrencyRecord::Usd,
            Currency::Aud => CurrencyRecord::Aud,
            Currency::Cad => CurrencyRecord::Cad,
            Currency::Nzd => CurrencyRecord::Nzd,
            Currency::Cny => CurrencyRecord::Cny,
            Currency::Brl => CurrencyRecord::Brl,
            Currency::Pln => CurrencyRecord::Pln,
            Currency::Try => CurrencyRecord::Try,
            Currency::Jpy => CurrencyRecord::Jpy,
            Currency::Czk => CurrencyRecord::Czk,
            Currency::Rub => CurrencyRecord::Rub,
            Currency::Aed => CurrencyRecord::Aed,
            Currency::Sar => CurrencyRecord::Sar,
            Currency::Hkd => CurrencyRecord::Hkd,
            Currency::Sgd => CurrencyRecord::Sgd,
            Currency::Chf => CurrencyRecord::Chf,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CurrencyRecord;
    use rstest::rstest;

    #[rstest]
    #[case(CurrencyRecord::Eur, "\"EUR\"")]
    #[case(CurrencyRecord::Gbp, "\"GBP\"")]
    #[case(CurrencyRecord::Usd, "\"USD\"")]
    #[case(CurrencyRecord::Aud, "\"AUD\"")]
    #[case(CurrencyRecord::Cad, "\"CAD\"")]
    #[case(CurrencyRecord::Nzd, "\"NZD\"")]
    #[case(CurrencyRecord::Cny, "\"CNY\"")]
    #[case(CurrencyRecord::Brl, "\"BRL\"")]
    #[case(CurrencyRecord::Pln, "\"PLN\"")]
    #[case(CurrencyRecord::Try, "\"TRY\"")]
    #[case(CurrencyRecord::Jpy, "\"JPY\"")]
    #[case(CurrencyRecord::Czk, "\"CZK\"")]
    #[case(CurrencyRecord::Rub, "\"RUB\"")]
    #[case(CurrencyRecord::Aed, "\"AED\"")]
    #[case(CurrencyRecord::Sar, "\"SAR\"")]
    #[case(CurrencyRecord::Hkd, "\"HKD\"")]
    #[case(CurrencyRecord::Sgd, "\"SGD\"")]
    #[case(CurrencyRecord::Chf, "\"CHF\"")]
    #[trace]
    fn should_serialize_currency_in_screaming_snake_case(
        #[case] currency: CurrencyRecord,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&currency).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"EUR\"", CurrencyRecord::Eur)]
    #[case("\"GBP\"", CurrencyRecord::Gbp)]
    #[case("\"USD\"", CurrencyRecord::Usd)]
    #[case("\"AUD\"", CurrencyRecord::Aud)]
    #[case("\"CAD\"", CurrencyRecord::Cad)]
    #[case("\"NZD\"", CurrencyRecord::Nzd)]
    #[case("\"CNY\"", CurrencyRecord::Cny)]
    #[case("\"BRL\"", CurrencyRecord::Brl)]
    #[case("\"PLN\"", CurrencyRecord::Pln)]
    #[case("\"TRY\"", CurrencyRecord::Try)]
    #[case("\"JPY\"", CurrencyRecord::Jpy)]
    #[case("\"CZK\"", CurrencyRecord::Czk)]
    #[case("\"RUB\"", CurrencyRecord::Rub)]
    #[case("\"AED\"", CurrencyRecord::Aed)]
    #[case("\"SAR\"", CurrencyRecord::Sar)]
    #[case("\"HKD\"", CurrencyRecord::Hkd)]
    #[case("\"SGD\"", CurrencyRecord::Sgd)]
    #[case("\"CHF\"", CurrencyRecord::Chf)]
    #[trace]
    fn should_deserialize_currency_in_screaming_snake_case(
        #[case] currency: &str,
        #[case] expected: CurrencyRecord,
    ) {
        let actual = serde_json::from_str::<CurrencyRecord>(currency).unwrap();
        assert_eq!(actual, expected);
    }
}
