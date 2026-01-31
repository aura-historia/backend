use crate::core::product_event::domain::ProductDomainEventPayload;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProductDomainEventTypeRecord {
    DomainCreated,
    DomainStateListed,
    DomainStateAvailable,
    DomainStateReserved,
    DomainStateSold,
    DomainStateRemoved,
    DomainStateUnknown,
    DomainPriceDiscovered,
    DomainPriceDropped,
    DomainPriceIncreased,
    DomainPriceRemoved,
}

impl From<&ProductDomainEventPayload> for ProductDomainEventTypeRecord {
    fn from(domain: &ProductDomainEventPayload) -> Self {
        match domain {
            ProductDomainEventPayload::Created(_) => ProductDomainEventTypeRecord::DomainCreated,
            ProductDomainEventPayload::StateListed(_) => {
                ProductDomainEventTypeRecord::DomainStateListed
            }
            ProductDomainEventPayload::StateAvailable(_) => {
                ProductDomainEventTypeRecord::DomainStateAvailable
            }
            ProductDomainEventPayload::StateReserved(_) => {
                ProductDomainEventTypeRecord::DomainStateReserved
            }
            ProductDomainEventPayload::StateSold(_) => {
                ProductDomainEventTypeRecord::DomainStateSold
            }
            ProductDomainEventPayload::StateRemoved(_) => {
                ProductDomainEventTypeRecord::DomainStateRemoved
            }
            ProductDomainEventPayload::StateUnknown(_) => {
                ProductDomainEventTypeRecord::DomainStateUnknown
            }
            ProductDomainEventPayload::PriceDiscovered(_) => {
                ProductDomainEventTypeRecord::DomainPriceDiscovered
            }
            ProductDomainEventPayload::PriceDropped(_) => {
                ProductDomainEventTypeRecord::DomainPriceDropped
            }
            ProductDomainEventPayload::PriceIncreased(_) => {
                ProductDomainEventTypeRecord::DomainPriceIncreased
            }
            ProductDomainEventPayload::PriceRemoved(_) => {
                ProductDomainEventTypeRecord::DomainPriceRemoved
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
    #[case(ProductDomainEventTypeRecord::DomainCreated, "\"DOMAIN_CREATED\"")]
    #[case(
        ProductDomainEventTypeRecord::DomainStateListed,
        "\"DOMAIN_STATE_LISTED\""
    )]
    #[case(
        ProductDomainEventTypeRecord::DomainStateAvailable,
        "\"DOMAIN_STATE_AVAILABLE\""
    )]
    #[case(
        ProductDomainEventTypeRecord::DomainStateReserved,
        "\"DOMAIN_STATE_RESERVED\""
    )]
    #[case(ProductDomainEventTypeRecord::DomainStateSold, "\"DOMAIN_STATE_SOLD\"")]
    #[case(
        ProductDomainEventTypeRecord::DomainStateRemoved,
        "\"DOMAIN_STATE_REMOVED\""
    )]
    #[case(
        ProductDomainEventTypeRecord::DomainStateUnknown,
        "\"DOMAIN_STATE_UNKNOWN\""
    )]
    #[case(
        ProductDomainEventTypeRecord::DomainPriceDiscovered,
        "\"DOMAIN_PRICE_DISCOVERED\""
    )]
    #[case(
        ProductDomainEventTypeRecord::DomainPriceDropped,
        "\"DOMAIN_PRICE_DROPPED\""
    )]
    #[case(
        ProductDomainEventTypeRecord::DomainPriceIncreased,
        "\"DOMAIN_PRICE_INCREASED\""
    )]
    fn should_serialize_product_event_type_record_in_screaming_snake_case(
        #[case] product_state_record: ProductDomainEventTypeRecord,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&product_state_record).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[trace]
    #[case("\"DOMAIN_CREATED\"", ProductDomainEventTypeRecord::DomainCreated)]
    #[case(
        "\"DOMAIN_STATE_LISTED\"",
        ProductDomainEventTypeRecord::DomainStateListed
    )]
    #[case(
        "\"DOMAIN_STATE_AVAILABLE\"",
        ProductDomainEventTypeRecord::DomainStateAvailable
    )]
    #[case(
        "\"DOMAIN_STATE_RESERVED\"",
        ProductDomainEventTypeRecord::DomainStateReserved
    )]
    #[case("\"DOMAIN_STATE_SOLD\"", ProductDomainEventTypeRecord::DomainStateSold)]
    #[case(
        "\"DOMAIN_STATE_REMOVED\"",
        ProductDomainEventTypeRecord::DomainStateRemoved
    )]
    #[case(
        "\"DOMAIN_STATE_UNKNOWN\"",
        ProductDomainEventTypeRecord::DomainStateUnknown
    )]
    #[case(
        "\"DOMAIN_PRICE_DISCOVERED\"",
        ProductDomainEventTypeRecord::DomainPriceDiscovered
    )]
    #[case(
        "\"DOMAIN_PRICE_DROPPED\"",
        ProductDomainEventTypeRecord::DomainPriceDropped
    )]
    #[case(
        "\"DOMAIN_PRICE_INCREASED\"",
        ProductDomainEventTypeRecord::DomainPriceIncreased
    )]
    fn should_deserialize_product_event_type_record_in_screaming_snake_case(
        #[case] currency: &str,
        #[case] expected: ProductDomainEventTypeRecord,
    ) {
        let actual = serde_json::from_str::<ProductDomainEventTypeRecord>(currency).unwrap();
        assert_eq!(actual, expected);
    }
}
