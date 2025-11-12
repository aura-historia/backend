use crate::dynamodb::product_state_record::ProductStateRecord;
use common::product_state::domain::ProductState;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ItemStateDocument {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}

impl From<ProductStateRecord> for ItemStateDocument {
    fn from(document: ProductStateRecord) -> Self {
        match document {
            ProductStateRecord::Listed => ItemStateDocument::Listed,
            ProductStateRecord::Available => ItemStateDocument::Available,
            ProductStateRecord::Reserved => ItemStateDocument::Reserved,
            ProductStateRecord::Sold => ItemStateDocument::Sold,
            ProductStateRecord::Removed => ItemStateDocument::Removed,
            ProductStateRecord::Unknown => ItemStateDocument::Unknown,
        }
    }
}

impl From<ProductState> for ItemStateDocument {
    fn from(value: ProductState) -> Self {
        match value {
            ProductState::Listed => ItemStateDocument::Listed,
            ProductState::Available => ItemStateDocument::Available,
            ProductState::Reserved => ItemStateDocument::Reserved,
            ProductState::Sold => ItemStateDocument::Sold,
            ProductState::Removed => ItemStateDocument::Removed,
            ProductState::Unknown => ItemStateDocument::Unknown,
        }
    }
}

impl From<ItemStateDocument> for ProductState {
    fn from(value: ItemStateDocument) -> Self {
        match value {
            ItemStateDocument::Listed => ProductState::Listed,
            ItemStateDocument::Available => ProductState::Available,
            ItemStateDocument::Reserved => ProductState::Reserved,
            ItemStateDocument::Sold => ProductState::Sold,
            ItemStateDocument::Removed => ProductState::Removed,
            ItemStateDocument::Unknown => ProductState::Unknown,
        }
    }
}

impl ItemStateDocument {
    pub fn as_str(&self) -> &'static str {
        match self {
            ItemStateDocument::Listed => "LISTED",
            ItemStateDocument::Available => "AVAILABLE",
            ItemStateDocument::Reserved => "RESERVED",
            ItemStateDocument::Sold => "SOLD",
            ItemStateDocument::Removed => "REMOVED",
            ItemStateDocument::Unknown => "UNKNOWN",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ItemStateDocument;
    use rstest::rstest;

    #[rstest]
    #[case(ItemStateDocument::Listed, "\"LISTED\"")]
    #[case(ItemStateDocument::Available, "\"AVAILABLE\"")]
    #[case(ItemStateDocument::Reserved, "\"RESERVED\"")]
    #[case(ItemStateDocument::Sold, "\"SOLD\"")]
    #[case(ItemStateDocument::Removed, "\"REMOVED\"")]
    #[case(ItemStateDocument::Unknown, "\"UNKNOWN\"")]
    fn should_serialize_item_state_document_in_screaming_snake_case(
        #[case] state: ItemStateDocument,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&state).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"LISTED\"", ItemStateDocument::Listed)]
    #[case("\"AVAILABLE\"", ItemStateDocument::Available)]
    #[case("\"RESERVED\"", ItemStateDocument::Reserved)]
    #[case("\"SOLD\"", ItemStateDocument::Sold)]
    #[case("\"REMOVED\"", ItemStateDocument::Removed)]
    #[case("\"UNKNOWN\"", ItemStateDocument::Unknown)]
    fn should_deserialize_item_state_document_in_screaming_snake_case(
        #[case] state: &str,
        #[case] expected: ItemStateDocument,
    ) {
        let actual = serde_json::from_str::<ItemStateDocument>(state).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case(ItemStateDocument::Listed)]
    #[case(ItemStateDocument::Available)]
    #[case(ItemStateDocument::Reserved)]
    #[case(ItemStateDocument::Sold)]
    #[case(ItemStateDocument::Removed)]
    #[case(ItemStateDocument::Unknown)]
    fn should_as_str_match_serialized(#[case] state: ItemStateDocument) {
        let serialized = serde_json::to_string::<ItemStateDocument>(&state)
            .unwrap()
            .replace("\"", "");
        let as_str = state.as_str();
        assert_eq!(serialized, as_str);
    }
}
