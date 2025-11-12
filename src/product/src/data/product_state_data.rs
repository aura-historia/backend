use common::product_state::domain::ProductState;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ItemStateData {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}

impl From<ProductState> for ItemStateData {
    fn from(domain: ProductState) -> Self {
        match domain {
            ProductState::Listed => ItemStateData::Listed,
            ProductState::Available => ItemStateData::Available,
            ProductState::Reserved => ItemStateData::Reserved,
            ProductState::Sold => ItemStateData::Sold,
            ProductState::Removed => ItemStateData::Removed,
            ProductState::Unknown => ItemStateData::Unknown,
        }
    }
}

impl From<ItemStateData> for ProductState {
    fn from(cmd: ItemStateData) -> Self {
        match cmd {
            ItemStateData::Listed => ProductState::Listed,
            ItemStateData::Available => ProductState::Available,
            ItemStateData::Reserved => ProductState::Reserved,
            ItemStateData::Sold => ProductState::Sold,
            ItemStateData::Removed => ProductState::Removed,
            ItemStateData::Unknown => ProductState::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ItemStateData;
    use rstest::rstest;

    #[rstest]
    #[case(ItemStateData::Listed, "\"LISTED\"")]
    #[case(ItemStateData::Available, "\"AVAILABLE\"")]
    #[case(ItemStateData::Reserved, "\"RESERVED\"")]
    #[case(ItemStateData::Sold, "\"SOLD\"")]
    #[case(ItemStateData::Removed, "\"REMOVED\"")]
    #[case(ItemStateData::Unknown, "\"UNKNOWN\"")]
    fn should_serialize_item_state_data_in_screaming_snake_case(
        #[case] item_state_record: ItemStateData,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&item_state_record).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"LISTED\"", ItemStateData::Listed)]
    #[case("\"AVAILABLE\"", ItemStateData::Available)]
    #[case("\"RESERVED\"", ItemStateData::Reserved)]
    #[case("\"SOLD\"", ItemStateData::Sold)]
    #[case("\"REMOVED\"", ItemStateData::Removed)]
    #[case("\"UNKNOWN\"", ItemStateData::Unknown)]
    fn should_deserialize_item_state_data_in_screaming_snake_case(
        #[case] currency: &str,
        #[case] expected: ItemStateData,
    ) {
        let actual = serde_json::from_str::<ItemStateData>(currency).unwrap();
        assert_eq!(actual, expected);
    }
}
