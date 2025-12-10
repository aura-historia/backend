use crate::core::sort_shop_field::SortShopField;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortShopFieldData {
    #[default]
    Score,
    Name,
    Updated,
    Created,
}

impl SortShopFieldData {
    pub fn as_str(&self) -> &'static str {
        match self {
            SortShopFieldData::Score => "score",
            SortShopFieldData::Name => "name",
            SortShopFieldData::Updated => "updated",
            SortShopFieldData::Created => "created",
        }
    }
}

impl From<SortShopFieldData> for &'static str {
    fn from(value: SortShopFieldData) -> Self {
        value.as_str()
    }
}

impl<'a> TryFrom<&'a str> for SortShopFieldData {
    type Error = String;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        match value {
            "score" => Ok(SortShopFieldData::Score),
            "name" => Ok(SortShopFieldData::Name),
            "updated" => Ok(SortShopFieldData::Updated),
            "created" => Ok(SortShopFieldData::Created),
            invalid => Err(format!(
                "Expected any of: 'score', 'name', 'updated', 'created'. Got: '{invalid}'"
            )),
        }
    }
}

impl From<SortShopFieldData> for SortShopField {
    fn from(value: SortShopFieldData) -> Self {
        match value {
            SortShopFieldData::Score => SortShopField::Score,
            SortShopFieldData::Name => SortShopField::Name,
            SortShopFieldData::Updated => SortShopField::Updated,
            SortShopFieldData::Created => SortShopField::Created,
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest;

    use crate::data::sort_shop_field_data::SortShopFieldData;

    #[trace]
    #[rstest::rstest]
    #[case(SortShopFieldData::Name)]
    #[case(SortShopFieldData::Created)]
    #[case(SortShopFieldData::Updated)]
    fn should_match_as_str_serialize(#[case] field: SortShopFieldData) {
        let serialized = serde_json::to_string(&field).unwrap().replace("\"", "");
        let as_str = field.as_str();

        assert_eq!(as_str, &serialized);
    }
}
