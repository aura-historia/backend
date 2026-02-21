use crate::period::core::LocalizedPeriod;
use common::{
    language::data::LocalizedTextData,
    period_key::{PeriodId, PeriodKey},
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetPeriodSummaryData {
    pub period_id: PeriodId,
    pub period_key: PeriodKey,
    pub name: LocalizedTextData,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl From<LocalizedPeriod> for GetPeriodSummaryData {
    fn from(period: LocalizedPeriod) -> Self {
        GetPeriodSummaryData {
            period_id: period.period_id,
            period_key: period.period_key,
            name: period.display_name.into(),
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
        let datum = GetPeriodSummaryData {
            period_id: "renaissance".into(),
            period_key: "renaissance-key".into(),
            name: LocalizedTextData::new("Renaissance", LanguageData::En),
            created: datetime!(2020 - 01 - 01 0:00 UTC),
            updated: datetime!(2020 - 06 - 01 0:00 UTC),
        };

        let expected = json!({
            "periodId": "renaissance",
            "periodKey": "renaissance-key",
            "name": { "text": "Renaissance", "language": "en" },
            "created": "2020-01-01T00:00:00Z",
            "updated": "2020-06-01T00:00:00Z",
        });

        let actual = serde_json::to_value(&datum).unwrap();

        assert_eq!(expected, actual);
    }
}
