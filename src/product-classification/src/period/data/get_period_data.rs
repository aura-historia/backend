use crate::period::core::LocalizedPeriod;
use common::{
    language::data::LocalizedTextData,
    period_key::{PeriodId, PeriodKey},
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetPeriodData {
    pub period_id: PeriodId,
    pub period_key: PeriodKey,
    pub name: LocalizedTextData,
    pub products: u32,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl GetPeriodData {
    pub fn from_period_with_product_count(period: LocalizedPeriod, products: u32) -> Self {
        GetPeriodData {
            period_id: period.period_id,
            period_key: period.period_key,
            name: period.display_name.into(),
            products,
            created: period.created,
            updated: period.updated,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::language::data::LanguageData;
    use serde_json::json;
    use time::macros::datetime;

    #[test]
    fn should_serialize_with_all_fields() {
        let datum = GetPeriodData {
            period_id: "renaissance".into(),
            period_key: "renaissance-key".into(),
            name: LocalizedTextData::new("Renaissance", LanguageData::En),
            products: 42,
            created: datetime!(2020 - 01 - 01 0:00 UTC),
            updated: datetime!(2020 - 06 - 01 0:00 UTC),
        };

        let expected = json!({
            "periodId": "renaissance",
            "periodKey": "renaissance-key",
            "name": { "text": "Renaissance", "language": "en" },
            "products": 42,
            "created": "2020-01-01T00:00:00Z",
            "updated": "2020-06-01T00:00:00Z",
        });

        let actual = serde_json::to_value(&datum).unwrap();

        assert_eq!(expected, actual);
    }
}

