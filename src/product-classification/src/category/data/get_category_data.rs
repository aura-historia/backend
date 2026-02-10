use crate::category::core::LocalizedCategory;
use common::{
    category_key::{CategoryId, CategoryKey},
    language::data::LocalizedTextData,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetCategoryData {
    pub category_id: CategoryId,
    pub category_key: CategoryKey,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<LocalizedTextData>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<LocalizedTextData>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl From<LocalizedCategory> for GetCategoryData {
    fn from(category: LocalizedCategory) -> Self {
        GetCategoryData {
            category_id: category.category_id,
            category_key: category.category_key,
            name: category.display_name.map(LocalizedTextData::from),
            description: category.display_description.map(LocalizedTextData::from),
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
        let datum = GetCategoryData {
            category_id: "furniture".into(),
            category_key: "furniture-key".into(),
            name: Some(LocalizedTextData::new("Furniture", LanguageData::En)),
            description: Some(LocalizedTextData::new("All furniture", LanguageData::En)),
            created: datetime!(2020 - 01 - 01 0:00 UTC),
            updated: datetime!(2020 - 06 - 01 0:00 UTC),
        };

        let expected = json!({
            "categoryId": "furniture",
            "categoryKey": "furniture-key",
            "name": { "text": "Furniture", "language": "en" },
            "description": { "text": "All furniture", "language": "en" },
            "created": "2020-01-01T00:00:00Z",
            "updated": "2020-06-01T00:00:00Z",
        });

        let actual = serde_json::to_value(&datum).unwrap();

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_serialize_without_optional_fields() {
        let datum = GetCategoryData {
            category_id: "furniture".into(),
            category_key: "furniture-key".into(),
            name: None,
            description: None,
            created: datetime!(2020 - 01 - 01 0:00 UTC),
            updated: datetime!(2020 - 06 - 01 0:00 UTC),
        };

        let actual = serde_json::to_value(&datum).unwrap();

        assert!(actual.get("name").is_none());
        assert!(actual.get("description").is_none());
    }
}
