use crate::dynamodb::product_state_record::ProductStateRecord;
use common::product_state::domain::ProductState;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProductStateDocument {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}

impl From<ProductStateRecord> for ProductStateDocument {
    fn from(document: ProductStateRecord) -> Self {
        match document {
            ProductStateRecord::Listed => ProductStateDocument::Listed,
            ProductStateRecord::Available => ProductStateDocument::Available,
            ProductStateRecord::Reserved => ProductStateDocument::Reserved,
            ProductStateRecord::Sold => ProductStateDocument::Sold,
            ProductStateRecord::Removed => ProductStateDocument::Removed,
            ProductStateRecord::Unknown => ProductStateDocument::Unknown,
        }
    }
}

impl From<ProductState> for ProductStateDocument {
    fn from(value: ProductState) -> Self {
        match value {
            ProductState::Listed => ProductStateDocument::Listed,
            ProductState::Available => ProductStateDocument::Available,
            ProductState::Reserved => ProductStateDocument::Reserved,
            ProductState::Sold => ProductStateDocument::Sold,
            ProductState::Removed => ProductStateDocument::Removed,
            ProductState::Unknown => ProductStateDocument::Unknown,
        }
    }
}

impl From<ProductStateDocument> for ProductState {
    fn from(value: ProductStateDocument) -> Self {
        match value {
            ProductStateDocument::Listed => ProductState::Listed,
            ProductStateDocument::Available => ProductState::Available,
            ProductStateDocument::Reserved => ProductState::Reserved,
            ProductStateDocument::Sold => ProductState::Sold,
            ProductStateDocument::Removed => ProductState::Removed,
            ProductStateDocument::Unknown => ProductState::Unknown,
        }
    }
}

impl ProductStateDocument {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProductStateDocument::Listed => "LISTED",
            ProductStateDocument::Available => "AVAILABLE",
            ProductStateDocument::Reserved => "RESERVED",
            ProductStateDocument::Sold => "SOLD",
            ProductStateDocument::Removed => "REMOVED",
            ProductStateDocument::Unknown => "UNKNOWN",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProductStateDocument;
    use rstest::rstest;

    #[rstest]
    #[trace]
    #[case(ProductStateDocument::Listed, "\"LISTED\"")]
    #[case(ProductStateDocument::Available, "\"AVAILABLE\"")]
    #[case(ProductStateDocument::Reserved, "\"RESERVED\"")]
    #[case(ProductStateDocument::Sold, "\"SOLD\"")]
    #[case(ProductStateDocument::Removed, "\"REMOVED\"")]
    #[case(ProductStateDocument::Unknown, "\"UNKNOWN\"")]
    fn should_serialize_product_state_document_in_screaming_snake_case(
        #[case] state: ProductStateDocument,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&state).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[trace]
    #[case("\"LISTED\"", ProductStateDocument::Listed)]
    #[case("\"AVAILABLE\"", ProductStateDocument::Available)]
    #[case("\"RESERVED\"", ProductStateDocument::Reserved)]
    #[case("\"SOLD\"", ProductStateDocument::Sold)]
    #[case("\"REMOVED\"", ProductStateDocument::Removed)]
    #[case("\"UNKNOWN\"", ProductStateDocument::Unknown)]
    fn should_deserialize_product_state_document_in_screaming_snake_case(
        #[case] state: &str,
        #[case] expected: ProductStateDocument,
    ) {
        let actual = serde_json::from_str::<ProductStateDocument>(state).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[trace]
    #[case(ProductStateDocument::Listed)]
    #[case(ProductStateDocument::Available)]
    #[case(ProductStateDocument::Reserved)]
    #[case(ProductStateDocument::Sold)]
    #[case(ProductStateDocument::Removed)]
    #[case(ProductStateDocument::Unknown)]
    fn should_as_str_match_serialized(#[case] state: ProductStateDocument) {
        let serialized = serde_json::to_string::<ProductStateDocument>(&state)
            .unwrap()
            .replace("\"", "");
        let as_str = state.as_str();
        assert_eq!(serialized, as_str);
    }
}
