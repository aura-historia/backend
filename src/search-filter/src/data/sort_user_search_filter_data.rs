use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortUserSearchFilterFieldData {
    Created,
}

impl SortUserSearchFilterFieldData {
    pub fn as_str(&self) -> &'static str {
        match self {
            SortUserSearchFilterFieldData::Created => "created",
        }
    }
}

impl From<SortUserSearchFilterFieldData> for &'static str {
    fn from(value: SortUserSearchFilterFieldData) -> Self {
        value.as_str()
    }
}

impl<'a> TryFrom<&'a str> for SortUserSearchFilterFieldData {
    type Error = String;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        match value {
            "created" => Ok(SortUserSearchFilterFieldData::Created),
            invalid => Err(format!("Expected any of: 'created'. Got: '{invalid}'")),
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest;

    use crate::data::sort_user_search_filter_data::SortUserSearchFilterFieldData;

    #[rstest::rstest]
    #[case(SortUserSearchFilterFieldData::Created)]
    #[trace]
    fn should_match_as_str_serialize(#[case] field: SortUserSearchFilterFieldData) {
        let serialized = serde_json::to_string(&field).unwrap().replace("\"", "");
        let as_str = field.as_str();

        assert_eq!(as_str, &serialized);
    }
}
