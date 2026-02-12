use crate::category::sort_category_field::SortCategoryField;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortCategoryFieldData {
    #[default]
    Score,
    Name,
    Updated,
    Created,
}

impl SortCategoryFieldData {
    pub fn as_str(&self) -> &'static str {
        match self {
            SortCategoryFieldData::Score => "score",
            SortCategoryFieldData::Name => "name",
            SortCategoryFieldData::Updated => "updated",
            SortCategoryFieldData::Created => "created",
        }
    }
}

impl From<SortCategoryFieldData> for &'static str {
    fn from(value: SortCategoryFieldData) -> Self {
        value.as_str()
    }
}

impl<'a> TryFrom<&'a str> for SortCategoryFieldData {
    type Error = String;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        match value {
            "score" => Ok(SortCategoryFieldData::Score),
            "name" => Ok(SortCategoryFieldData::Name),
            "updated" => Ok(SortCategoryFieldData::Updated),
            "created" => Ok(SortCategoryFieldData::Created),
            invalid => Err(format!(
                "Expected any of: 'score', 'name', 'updated', 'created'. Got: '{invalid}'"
            )),
        }
    }
}

impl From<SortCategoryFieldData> for SortCategoryField {
    fn from(value: SortCategoryFieldData) -> Self {
        match value {
            SortCategoryFieldData::Score => SortCategoryField::Score,
            SortCategoryFieldData::Name => SortCategoryField::Name,
            SortCategoryFieldData::Updated => SortCategoryField::Updated,
            SortCategoryFieldData::Created => SortCategoryField::Created,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::category::data::sort_category_field_data::SortCategoryFieldData;

    #[rstest::rstest]
    #[case(SortCategoryFieldData::Name)]
    #[case(SortCategoryFieldData::Created)]
    #[case(SortCategoryFieldData::Updated)]
    #[trace]
    fn should_match_as_str_serialize(#[case] field: SortCategoryFieldData) {
        let serialized = serde_json::to_string(&field).unwrap().replace("\"", "");
        let as_str = field.as_str();

        assert_eq!(as_str, &serialized);
    }
}
