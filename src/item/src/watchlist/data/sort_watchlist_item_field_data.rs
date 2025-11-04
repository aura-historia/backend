use crate::watchlist::service::sort_watchlist_item_field::SortWatchlistItemField;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortWatchlistItemFieldData {
    Created,
}

impl SortWatchlistItemFieldData {
    pub fn as_str(&self) -> &'static str {
        match self {
            SortWatchlistItemFieldData::Created => "created",
        }
    }
}

impl From<SortWatchlistItemFieldData> for &'static str {
    fn from(value: SortWatchlistItemFieldData) -> Self {
        value.as_str()
    }
}

impl<'a> TryFrom<&'a str> for SortWatchlistItemFieldData {
    type Error = String;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        match value {
            "created" => Ok(SortWatchlistItemFieldData::Created),
            invalid => Err(format!("Expected any of: 'created'. Got: '{invalid}'")),
        }
    }
}

impl From<SortWatchlistItemFieldData> for SortWatchlistItemField {
    fn from(value: SortWatchlistItemFieldData) -> Self {
        match value {
            SortWatchlistItemFieldData::Created => SortWatchlistItemField::Created,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::watchlist::data::sort_watchlist_item_field_data::SortWatchlistItemFieldData;

    #[rstest::rstest]
    #[case(SortWatchlistItemFieldData::Created)]
    fn should_match_as_str_serialize(#[case] field: SortWatchlistItemFieldData) {
        let serialized = serde_json::to_string(&field).unwrap().replace("\"", "");
        let as_str = field.as_str();

        assert_eq!(as_str, &serialized);
    }
}
