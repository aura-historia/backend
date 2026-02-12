use crate::category::core::LocalizedCategory;
use common::{
    category_key::{CategoryId, CategoryKey},
    language::data::LocalizedTextData,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetCategorySummaryData {
    pub category_id: CategoryId,
    pub category_key: CategoryKey,
    pub name: LocalizedTextData,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl From<LocalizedCategory> for GetCategorySummaryData {
    fn from(category: LocalizedCategory) -> Self {
        GetCategorySummaryData {
            category_id: category.category_id,
            category_key: category.category_key,
            name: category.display_name.into(),
            created: category.created,
            updated: category.updated,
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
        let datum = GetCategorySummaryData {
            category_id: "furniture".into(),
            category_key: "furniture-key".into(),
            name: LocalizedTextData::new("Furniture", LanguageData::En),
            created: datetime!(2020 - 01 - 01 0:00 UTC),
            updated: datetime!(2020 - 06 - 01 0:00 UTC),
        };

        let expected = json!({
            "categoryId": "furniture",
            "categoryKey": "furniture-key",
            "name": { "text": "Furniture", "language": "en" },
            "created": "2020-01-01T00:00:00Z",
            "updated": "2020-06-01T00:00:00Z",
        });

        let actual = serde_json::to_value(&datum).unwrap();

        assert_eq!(expected, actual);
    }
}
