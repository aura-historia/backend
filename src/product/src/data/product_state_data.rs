use common::product_state::domain::ProductState;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProductStateData {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}

impl From<ProductState> for ProductStateData {
    fn from(domain: ProductState) -> Self {
        match domain {
            ProductState::Listed => ProductStateData::Listed,
            ProductState::Available => ProductStateData::Available,
            ProductState::Reserved => ProductStateData::Reserved,
            ProductState::Sold => ProductStateData::Sold,
            ProductState::Removed => ProductStateData::Removed,
            ProductState::Unknown => ProductStateData::Unknown,
        }
    }
}

impl From<ProductStateData> for ProductState {
    fn from(cmd: ProductStateData) -> Self {
        match cmd {
            ProductStateData::Listed => ProductState::Listed,
            ProductStateData::Available => ProductState::Available,
            ProductStateData::Reserved => ProductState::Reserved,
            ProductStateData::Sold => ProductState::Sold,
            ProductStateData::Removed => ProductState::Removed,
            ProductStateData::Unknown => ProductState::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProductStateData;
    use rstest::rstest;

    #[rstest]
    #[case(ProductStateData::Listed, "\"LISTED\"")]
    #[case(ProductStateData::Available, "\"AVAILABLE\"")]
    #[case(ProductStateData::Reserved, "\"RESERVED\"")]
    #[case(ProductStateData::Sold, "\"SOLD\"")]
    #[case(ProductStateData::Removed, "\"REMOVED\"")]
    #[case(ProductStateData::Unknown, "\"UNKNOWN\"")]
    fn should_serialize_product_state_data_in_screaming_snake_case(
        #[case] item_state_record: ProductStateData,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&item_state_record).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"LISTED\"", ProductStateData::Listed)]
    #[case("\"AVAILABLE\"", ProductStateData::Available)]
    #[case("\"RESERVED\"", ProductStateData::Reserved)]
    #[case("\"SOLD\"", ProductStateData::Sold)]
    #[case("\"REMOVED\"", ProductStateData::Removed)]
    #[case("\"UNKNOWN\"", ProductStateData::Unknown)]
    fn should_deserialize_product_state_data_in_screaming_snake_case(
        #[case] currency: &str,
        #[case] expected: ProductStateData,
    ) {
        let actual = serde_json::from_str::<ProductStateData>(currency).unwrap();
        assert_eq!(actual, expected);
    }
}
