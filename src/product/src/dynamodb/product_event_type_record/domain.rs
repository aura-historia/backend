use crate::core::product_event::domain::ProductDomainEventPayload;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProductDomainEventTypeRecord {
    DomainCreated,
    DomainStateChanged,
    DomainPriceChanged,
}

impl From<&ProductDomainEventPayload> for ProductDomainEventTypeRecord {
    fn from(domain: &ProductDomainEventPayload) -> Self {
        match domain {
            ProductDomainEventPayload::Created(_) => ProductDomainEventTypeRecord::DomainCreated,
            ProductDomainEventPayload::StateChanged(_) => {
                ProductDomainEventTypeRecord::DomainStateChanged
            }
            ProductDomainEventPayload::PriceChanged(_) => {
                ProductDomainEventTypeRecord::DomainPriceChanged
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
        ProductDomainEventTypeRecord::DomainStateChanged,
        "\"DOMAIN_STATE_CHANGED\""
    )]
    #[case(
        ProductDomainEventTypeRecord::DomainPriceChanged,
        "\"DOMAIN_PRICE_CHANGED\""
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
        "\"DOMAIN_STATE_CHANGED\"",
        ProductDomainEventTypeRecord::DomainStateChanged
    )]
    #[case(
        "\"DOMAIN_PRICE_CHANGED\"",
        ProductDomainEventTypeRecord::DomainPriceChanged
    )]
    fn should_deserialize_product_event_type_record_in_screaming_snake_case(
        #[case] currency: &str,
        #[case] expected: ProductDomainEventTypeRecord,
    ) {
        let actual = serde_json::from_str::<ProductDomainEventTypeRecord>(currency).unwrap();
        assert_eq!(actual, expected);
    }
}
