use crate::service::sort_watchlist_product_field::SortWatchlistProductField;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortWatchlistProductFieldData {
    Created,
}

impl SortWatchlistProductFieldData {
    pub fn as_str(&self) -> &'static str {
        match self {
            SortWatchlistProductFieldData::Created => "created",
        }
    }
}

impl From<SortWatchlistProductFieldData> for &'static str {
    fn from(value: SortWatchlistProductFieldData) -> Self {
        value.as_str()
    }
}

impl<'a> TryFrom<&'a str> for SortWatchlistProductFieldData {
    type Error = String;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        match value {
            "created" => Ok(SortWatchlistProductFieldData::Created),
            invalid => Err(format!("Expected any of: 'created'. Got: '{invalid}'")),
        }
    }
}

impl From<SortWatchlistProductFieldData> for SortWatchlistProductField {
    fn from(value: SortWatchlistProductFieldData) -> Self {
        match value {
            SortWatchlistProductFieldData::Created => SortWatchlistProductField::Created,
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest;

    use crate::data::sort_watchlist_product_field_data::SortWatchlistProductFieldData;

    #[rstest::rstest]
    #[case(SortWatchlistProductFieldData::Created)]
    #[trace]
    fn should_match_as_str_serialize(#[case] field: SortWatchlistProductFieldData) {
        let serialized = serde_json::to_string(&field).unwrap().replace("\"", "");
        let as_str = field.as_str();

        assert_eq!(as_str, &serialized);
    }
}
