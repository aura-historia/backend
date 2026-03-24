use crate::service::sort_search_filter_match_field::SortSearchFilterMatchField;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortSearchFilterMatchFieldData {
    Created,
}

impl SortSearchFilterMatchFieldData {
    pub fn as_str(&self) -> &'static str {
        match self {
            SortSearchFilterMatchFieldData::Created => "created",
        }
    }
}

impl From<SortSearchFilterMatchFieldData> for &'static str {
    fn from(value: SortSearchFilterMatchFieldData) -> Self {
        value.as_str()
    }
}

impl<'a> TryFrom<&'a str> for SortSearchFilterMatchFieldData {
    type Error = String;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        match value {
            "created" => Ok(SortSearchFilterMatchFieldData::Created),
            invalid => Err(format!("Expected any of: 'created'. Got: '{invalid}'")),
        }
    }
}

impl From<SortSearchFilterMatchFieldData> for SortSearchFilterMatchField {
    fn from(value: SortSearchFilterMatchFieldData) -> Self {
        match value {
            SortSearchFilterMatchFieldData::Created => SortSearchFilterMatchField::Created,
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest;

    use crate::data::sort_search_filter_match_field_data::SortSearchFilterMatchFieldData;

    #[rstest::rstest]
    #[case(SortSearchFilterMatchFieldData::Created)]
    #[trace]
    fn should_match_as_str_serialize(#[case] field: SortSearchFilterMatchFieldData) {
        let serialized = serde_json::to_string(&field).unwrap().replace("\"", "");
        let as_str = field.as_str();

        assert_eq!(as_str, &serialized);
    }
}
