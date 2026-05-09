use crate::core::sort_product_field::SortProductField;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortProductFieldData {
    #[default]
    Score,
    Price,
    Updated,
    Created,
}

impl SortProductFieldData {
    pub fn as_str(&self) -> &'static str {
        match self {
            SortProductFieldData::Score => "score",
            SortProductFieldData::Price => "price",
            SortProductFieldData::Updated => "updated",
            SortProductFieldData::Created => "created",
        }
    }
}

impl From<SortProductFieldData> for &'static str {
    fn from(value: SortProductFieldData) -> Self {
        value.as_str()
    }
}

impl<'a> TryFrom<&'a str> for SortProductFieldData {
    type Error = String;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        match value {
            "score" => Ok(SortProductFieldData::Score),
            "price" => Ok(SortProductFieldData::Price),
            "updated" => Ok(SortProductFieldData::Updated),
            "created" => Ok(SortProductFieldData::Created),
            invalid => Err(format!(
                "Expected any of: 'score', 'price', 'updated', 'created'. Got: '{invalid}'"
            )),
        }
    }
}

impl From<SortProductFieldData> for SortProductField {
    fn from(value: SortProductFieldData) -> Self {
        match value {
            SortProductFieldData::Score => SortProductField::Score,
            SortProductFieldData::Price => SortProductField::Price,
            SortProductFieldData::Updated => SortProductField::Updated,
            SortProductFieldData::Created => SortProductField::Created,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::data::sort_product_field_data::SortProductFieldData;
    use rstest;

    #[rstest::rstest]
    #[case(SortProductFieldData::Score)]
    #[case(SortProductFieldData::Price)]
    #[case(SortProductFieldData::Created)]
    #[case(SortProductFieldData::Updated)]
    #[trace]
    fn should_match_as_str_serialize(#[case] field: SortProductFieldData) {
        let serialized = serde_json::to_string(&field).unwrap().replace("\"", "");
        let as_str = field.as_str();

        assert_eq!(as_str, &serialized);
    }
}
