use crate::core::product_event::ItemEventPayload;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProductEventTypeRecord {
    Created,
    StateListed,
    StateAvailable,
    StateReserved,
    StateSold,
    StateRemoved,
    StateUnknown,
    PriceDiscovered,
    PriceDropped,
    PriceIncreased,
    PriceRemoved,
}

impl From<&ItemEventPayload> for ProductEventTypeRecord {
    fn from(domain: &ItemEventPayload) -> Self {
        match domain {
            ItemEventPayload::Created(_) => ProductEventTypeRecord::Created,
            ItemEventPayload::StateListed(_) => ProductEventTypeRecord::StateListed,
            ItemEventPayload::StateAvailable(_) => ProductEventTypeRecord::StateAvailable,
            ItemEventPayload::StateReserved(_) => ProductEventTypeRecord::StateReserved,
            ItemEventPayload::StateSold(_) => ProductEventTypeRecord::StateSold,
            ItemEventPayload::StateRemoved(_) => ProductEventTypeRecord::StateRemoved,
            ItemEventPayload::StateUnknown(_) => ProductEventTypeRecord::StateUnknown,
            ItemEventPayload::PriceDiscovered(_) => ProductEventTypeRecord::PriceDiscovered,
            ItemEventPayload::PriceDropped(_) => ProductEventTypeRecord::PriceDropped,
            ItemEventPayload::PriceIncreased(_) => ProductEventTypeRecord::PriceIncreased,
            ItemEventPayload::PriceRemoved(_) => ProductEventTypeRecord::PriceRemoved,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProductEventTypeRecord;
    use rstest::rstest;

    #[rstest]
    #[case(ProductEventTypeRecord::Created, "\"CREATED\"")]
    #[case(ProductEventTypeRecord::StateListed, "\"STATE_LISTED\"")]
    #[case(ProductEventTypeRecord::StateAvailable, "\"STATE_AVAILABLE\"")]
    #[case(ProductEventTypeRecord::StateReserved, "\"STATE_RESERVED\"")]
    #[case(ProductEventTypeRecord::StateSold, "\"STATE_SOLD\"")]
    #[case(ProductEventTypeRecord::StateRemoved, "\"STATE_REMOVED\"")]
    #[case(ProductEventTypeRecord::StateUnknown, "\"STATE_UNKNOWN\"")]
    #[case(ProductEventTypeRecord::PriceDiscovered, "\"PRICE_DISCOVERED\"")]
    #[case(ProductEventTypeRecord::PriceDropped, "\"PRICE_DROPPED\"")]
    #[case(ProductEventTypeRecord::PriceIncreased, "\"PRICE_INCREASED\"")]
    fn should_serialize_item_event_type_record_in_screaming_snake_case(
        #[case] item_state_record: ProductEventTypeRecord,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&item_state_record).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"CREATED\"", ProductEventTypeRecord::Created)]
    #[case("\"STATE_LISTED\"", ProductEventTypeRecord::StateListed)]
    #[case("\"STATE_AVAILABLE\"", ProductEventTypeRecord::StateAvailable)]
    #[case("\"STATE_RESERVED\"", ProductEventTypeRecord::StateReserved)]
    #[case("\"STATE_SOLD\"", ProductEventTypeRecord::StateSold)]
    #[case("\"STATE_REMOVED\"", ProductEventTypeRecord::StateRemoved)]
    #[case("\"STATE_UNKNOWN\"", ProductEventTypeRecord::StateUnknown)]
    #[case("\"PRICE_DISCOVERED\"", ProductEventTypeRecord::PriceDiscovered)]
    #[case("\"PRICE_DROPPED\"", ProductEventTypeRecord::PriceDropped)]
    #[case("\"PRICE_INCREASED\"", ProductEventTypeRecord::PriceIncreased)]
    fn should_deserialize_item_event_type_record_in_screaming_snake_case(
        #[case] currency: &str,
        #[case] expected: ProductEventTypeRecord,
    ) {
        let actual = serde_json::from_str::<ProductEventTypeRecord>(currency).unwrap();
        assert_eq!(actual, expected);
    }
}
