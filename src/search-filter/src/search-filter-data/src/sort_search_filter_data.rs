use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortSearchFilterFieldData {
    Created,
}

impl SortSearchFilterFieldData {
    pub fn as_str(&self) -> &'static str {
        match self {
            SortSearchFilterFieldData::Created => "created",
        }
    }
}

impl From<SortSearchFilterFieldData> for &'static str {
    fn from(value: SortSearchFilterFieldData) -> Self {
        value.as_str()
    }
}

impl<'a> TryFrom<&'a str> for SortSearchFilterFieldData {
    type Error = String;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        match value {
            "created" => Ok(SortSearchFilterFieldData::Created),
            invalid => Err(format!("Expected any of: 'created'. Got: '{invalid}'")),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::sort_search_filter_data::SortSearchFilterFieldData;

    #[rstest::rstest]
    #[case(SortSearchFilterFieldData::Created)]
    fn should_match_as_str_serialize(#[case] field: SortSearchFilterFieldData) {
        let serialized = serde_json::to_string(&field).unwrap().replace("\"", "");
        let as_str = field.as_str();

        assert_eq!(as_str, &serialized);
    }
}
