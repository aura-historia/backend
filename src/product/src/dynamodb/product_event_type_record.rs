use crate::core::product_event::domain::ProductDomainEventPayload;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProductDomainEventTypeRecord {
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

impl From<&ProductDomainEventPayload> for ProductDomainEventTypeRecord {
    fn from(domain: &ProductDomainEventPayload) -> Self {
        match domain {
            ProductDomainEventPayload::Created(_) => ProductDomainEventTypeRecord::Created,
            ProductDomainEventPayload::StateListed(_) => ProductDomainEventTypeRecord::StateListed,
            ProductDomainEventPayload::StateAvailable(_) => {
                ProductDomainEventTypeRecord::StateAvailable
            }
            ProductDomainEventPayload::StateReserved(_) => {
                ProductDomainEventTypeRecord::StateReserved
            }
            ProductDomainEventPayload::StateSold(_) => ProductDomainEventTypeRecord::StateSold,
            ProductDomainEventPayload::StateRemoved(_) => {
                ProductDomainEventTypeRecord::StateRemoved
            }
            ProductDomainEventPayload::StateUnknown(_) => {
                ProductDomainEventTypeRecord::StateUnknown
            }
            ProductDomainEventPayload::PriceDiscovered(_) => {
                ProductDomainEventTypeRecord::PriceDiscovered
            }
            ProductDomainEventPayload::PriceDropped(_) => {
                ProductDomainEventTypeRecord::PriceDropped
            }
            ProductDomainEventPayload::PriceIncreased(_) => {
                ProductDomainEventTypeRecord::PriceIncreased
            }
            ProductDomainEventPayload::PriceRemoved(_) => {
                ProductDomainEventTypeRecord::PriceRemoved
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProductDomainEventTypeRecord;
    use rstest::rstest;

    #[rstest]
    #[trace]
    #[case(ProductDomainEventTypeRecord::Created, "\"CREATED\"")]
    #[case(ProductDomainEventTypeRecord::StateListed, "\"STATE_LISTED\"")]
    #[case(ProductDomainEventTypeRecord::StateAvailable, "\"STATE_AVAILABLE\"")]
    #[case(ProductDomainEventTypeRecord::StateReserved, "\"STATE_RESERVED\"")]
    #[case(ProductDomainEventTypeRecord::StateSold, "\"STATE_SOLD\"")]
    #[case(ProductDomainEventTypeRecord::StateRemoved, "\"STATE_REMOVED\"")]
    #[case(ProductDomainEventTypeRecord::StateUnknown, "\"STATE_UNKNOWN\"")]
    #[case(ProductDomainEventTypeRecord::PriceDiscovered, "\"PRICE_DISCOVERED\"")]
    #[case(ProductDomainEventTypeRecord::PriceDropped, "\"PRICE_DROPPED\"")]
    #[case(ProductDomainEventTypeRecord::PriceIncreased, "\"PRICE_INCREASED\"")]
    fn should_serialize_product_event_type_record_in_screaming_snake_case(
        #[case] product_state_record: ProductDomainEventTypeRecord,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&product_state_record).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[trace]
    #[case("\"CREATED\"", ProductDomainEventTypeRecord::Created)]
    #[case("\"STATE_LISTED\"", ProductDomainEventTypeRecord::StateListed)]
    #[case("\"STATE_AVAILABLE\"", ProductDomainEventTypeRecord::StateAvailable)]
    #[case("\"STATE_RESERVED\"", ProductDomainEventTypeRecord::StateReserved)]
    #[case("\"STATE_SOLD\"", ProductDomainEventTypeRecord::StateSold)]
    #[case("\"STATE_REMOVED\"", ProductDomainEventTypeRecord::StateRemoved)]
    #[case("\"STATE_UNKNOWN\"", ProductDomainEventTypeRecord::StateUnknown)]
    #[case("\"PRICE_DISCOVERED\"", ProductDomainEventTypeRecord::PriceDiscovered)]
    #[case("\"PRICE_DROPPED\"", ProductDomainEventTypeRecord::PriceDropped)]
    #[case("\"PRICE_INCREASED\"", ProductDomainEventTypeRecord::PriceIncreased)]
    fn should_deserialize_product_event_type_record_in_screaming_snake_case(
        #[case] currency: &str,
        #[case] expected: ProductDomainEventTypeRecord,
    ) {
        let actual = serde_json::from_str::<ProductDomainEventTypeRecord>(currency).unwrap();
        assert_eq!(actual, expected);
    }
}
