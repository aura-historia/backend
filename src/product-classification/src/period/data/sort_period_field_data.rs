use crate::period::sort_period_field::SortPeriodField;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortPeriodFieldData {
    #[default]
    Score,
    Name,
    Updated,
    Created,
}

impl SortPeriodFieldData {
    pub fn as_str(&self) -> &'static str {
        match self {
            SortPeriodFieldData::Score => "score",
            SortPeriodFieldData::Name => "name",
            SortPeriodFieldData::Updated => "updated",
            SortPeriodFieldData::Created => "created",
        }
    }
}

impl From<SortPeriodFieldData> for &'static str {
    fn from(value: SortPeriodFieldData) -> Self {
        value.as_str()
    }
}

impl<'a> TryFrom<&'a str> for SortPeriodFieldData {
    type Error = String;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        match value {
            "score" => Ok(SortPeriodFieldData::Score),
            "name" => Ok(SortPeriodFieldData::Name),
            "updated" => Ok(SortPeriodFieldData::Updated),
            "created" => Ok(SortPeriodFieldData::Created),
            invalid => Err(format!(
                "Expected any of: 'score', 'name', 'updated', 'created'. Got: '{invalid}'"
            )),
        }
    }
}

impl From<SortPeriodFieldData> for SortPeriodField {
    fn from(value: SortPeriodFieldData) -> Self {
        match value {
            SortPeriodFieldData::Score => SortPeriodField::Score,
            SortPeriodFieldData::Name => SortPeriodField::Name,
            SortPeriodFieldData::Updated => SortPeriodField::Updated,
            SortPeriodFieldData::Created => SortPeriodField::Created,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::period::data::sort_period_field_data::SortPeriodFieldData;

    #[rstest::rstest]
    #[case(SortPeriodFieldData::Name)]
    #[case(SortPeriodFieldData::Created)]
    #[case(SortPeriodFieldData::Updated)]
    #[trace]
    fn should_match_as_str_serialize(#[case] field: SortPeriodFieldData) {
        let serialized = serde_json::to_string(&field).unwrap().replace("\"", "");
        let as_str = field.as_str();

        assert_eq!(as_str, &serialized);
    }
}
