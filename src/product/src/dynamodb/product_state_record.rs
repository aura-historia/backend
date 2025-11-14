use common::product_state::domain::ProductState;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize, Hash)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProductStateRecord {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}

impl From<ProductState> for ProductStateRecord {
    fn from(domain: ProductState) -> Self {
        match domain {
            ProductState::Listed => ProductStateRecord::Listed,
            ProductState::Available => ProductStateRecord::Available,
            ProductState::Reserved => ProductStateRecord::Reserved,
            ProductState::Sold => ProductStateRecord::Sold,
            ProductState::Removed => ProductStateRecord::Removed,
            ProductState::Unknown => ProductStateRecord::Unknown,
        }
    }
}

impl From<ProductStateRecord> for ProductState {
    fn from(cmd: ProductStateRecord) -> Self {
        match cmd {
            ProductStateRecord::Listed => ProductState::Listed,
            ProductStateRecord::Available => ProductState::Available,
            ProductStateRecord::Reserved => ProductState::Reserved,
            ProductStateRecord::Sold => ProductState::Sold,
            ProductStateRecord::Removed => ProductState::Removed,
            ProductStateRecord::Unknown => ProductState::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProductStateRecord;
    use rstest::rstest;

    #[rstest]
    #[case(ProductStateRecord::Listed, "\"LISTED\"")]
    #[case(ProductStateRecord::Available, "\"AVAILABLE\"")]
    #[case(ProductStateRecord::Reserved, "\"RESERVED\"")]
    #[case(ProductStateRecord::Sold, "\"SOLD\"")]
    #[case(ProductStateRecord::Removed, "\"REMOVED\"")]
    #[case(ProductStateRecord::Unknown, "\"UNKNOWN\"")]
    fn should_serialize_product_state_record_in_screaming_snake_case(
        #[case] product_state_record: ProductStateRecord,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&product_state_record).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"LISTED\"", ProductStateRecord::Listed)]
    #[case("\"AVAILABLE\"", ProductStateRecord::Available)]
    #[case("\"RESERVED\"", ProductStateRecord::Reserved)]
    #[case("\"SOLD\"", ProductStateRecord::Sold)]
    #[case("\"REMOVED\"", ProductStateRecord::Removed)]
    #[case("\"UNKNOWN\"", ProductStateRecord::Unknown)]
    fn should_deserialize_product_state_record_in_screaming_snake_case(
        #[case] currency: &str,
        #[case] expected: ProductStateRecord,
    ) {
        let actual = serde_json::from_str::<ProductStateRecord>(currency).unwrap();
        assert_eq!(actual, expected);
    }
}
