use crate::core::product_event::domain::ProductDomainEventPayload;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProductDomainEventTypeRecord {
    DomainCreated,
    DomainStateChanged,
    DomainPriceChanged,
    DomainEstimatePriceChanged,
    DomainUrlChanged,
    DomainImagesChanged,
    DomainAuctionTimeChanged,
    DomainOriginYearChanged,
    DomainAuthenticityChanged,
    DomainConditionChanged,
    DomainProvenanceChanged,
    DomainRestorationChanged,
}

impl ProductDomainEventTypeRecord {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProductDomainEventTypeRecord::DomainCreated => "DOMAIN_CREATED",
            ProductDomainEventTypeRecord::DomainStateChanged => "DOMAIN_STATE_CHANGED",
            ProductDomainEventTypeRecord::DomainPriceChanged => "DOMAIN_PRICE_CHANGED",
            ProductDomainEventTypeRecord::DomainEstimatePriceChanged => {
                "DOMAIN_ESTIMATE_PRICE_CHANGED"
            }
            ProductDomainEventTypeRecord::DomainUrlChanged => "DOMAIN_URL_CHANGED",
            ProductDomainEventTypeRecord::DomainImagesChanged => "DOMAIN_IMAGES_CHANGED",
            ProductDomainEventTypeRecord::DomainAuctionTimeChanged => "DOMAIN_AUCTION_TIME_CHANGED",
            ProductDomainEventTypeRecord::DomainOriginYearChanged => "DOMAIN_ORIGIN_YEAR_CHANGED",
            ProductDomainEventTypeRecord::DomainAuthenticityChanged => {
                "DOMAIN_AUTHENTICITY_CHANGED"
            }
            ProductDomainEventTypeRecord::DomainConditionChanged => "DOMAIN_CONDITION_CHANGED",
            ProductDomainEventTypeRecord::DomainProvenanceChanged => "DOMAIN_PROVENANCE_CHANGED",
            ProductDomainEventTypeRecord::DomainRestorationChanged => "DOMAIN_RESTORATION_CHANGED",
        }
    }
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
            ProductDomainEventPayload::EstimatePriceChanged(_) => {
                ProductDomainEventTypeRecord::DomainEstimatePriceChanged
            }
            ProductDomainEventPayload::UrlChanged(_) => {
                ProductDomainEventTypeRecord::DomainUrlChanged
            }
            ProductDomainEventPayload::ImagesChanged(_) => {
                ProductDomainEventTypeRecord::DomainImagesChanged
            }
            ProductDomainEventPayload::AuctionTimeChanged(_) => {
                ProductDomainEventTypeRecord::DomainAuctionTimeChanged
            }
            ProductDomainEventPayload::OriginYearChanged(_) => {
                ProductDomainEventTypeRecord::DomainOriginYearChanged
            }
            ProductDomainEventPayload::AuthenticityChanged(_) => {
                ProductDomainEventTypeRecord::DomainAuthenticityChanged
            }
            ProductDomainEventPayload::ConditionChanged(_) => {
                ProductDomainEventTypeRecord::DomainConditionChanged
            }
            ProductDomainEventPayload::ProvenanceChanged(_) => {
                ProductDomainEventTypeRecord::DomainProvenanceChanged
            }
            ProductDomainEventPayload::RestorationChanged(_) => {
                ProductDomainEventTypeRecord::DomainRestorationChanged
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
    #[case(
        ProductDomainEventTypeRecord::DomainEstimatePriceChanged,
        "\"DOMAIN_ESTIMATE_PRICE_CHANGED\""
    )]
    #[case(
        ProductDomainEventTypeRecord::DomainUrlChanged,
        "\"DOMAIN_URL_CHANGED\""
    )]
    #[case(
        ProductDomainEventTypeRecord::DomainImagesChanged,
        "\"DOMAIN_IMAGES_CHANGED\""
    )]
    #[case(
        ProductDomainEventTypeRecord::DomainAuctionTimeChanged,
        "\"DOMAIN_AUCTION_TIME_CHANGED\""
    )]
    #[case(
        ProductDomainEventTypeRecord::DomainOriginYearChanged,
        "\"DOMAIN_ORIGIN_YEAR_CHANGED\""
    )]
    #[case(
        ProductDomainEventTypeRecord::DomainAuthenticityChanged,
        "\"DOMAIN_AUTHENTICITY_CHANGED\""
    )]
    #[case(
        ProductDomainEventTypeRecord::DomainConditionChanged,
        "\"DOMAIN_CONDITION_CHANGED\""
    )]
    #[case(
        ProductDomainEventTypeRecord::DomainProvenanceChanged,
        "\"DOMAIN_PROVENANCE_CHANGED\""
    )]
    #[case(
        ProductDomainEventTypeRecord::DomainRestorationChanged,
        "\"DOMAIN_RESTORATION_CHANGED\""
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
    #[case(
        "\"DOMAIN_ESTIMATE_PRICE_CHANGED\"",
        ProductDomainEventTypeRecord::DomainEstimatePriceChanged
    )]
    #[case(
        "\"DOMAIN_URL_CHANGED\"",
        ProductDomainEventTypeRecord::DomainUrlChanged
    )]
    #[case(
        "\"DOMAIN_IMAGES_CHANGED\"",
        ProductDomainEventTypeRecord::DomainImagesChanged
    )]
    #[case(
        "\"DOMAIN_AUCTION_TIME_CHANGED\"",
        ProductDomainEventTypeRecord::DomainAuctionTimeChanged
    )]
    #[case(
        "\"DOMAIN_ORIGIN_YEAR_CHANGED\"",
        ProductDomainEventTypeRecord::DomainOriginYearChanged
    )]
    #[case(
        "\"DOMAIN_AUTHENTICITY_CHANGED\"",
        ProductDomainEventTypeRecord::DomainAuthenticityChanged
    )]
    #[case(
        "\"DOMAIN_CONDITION_CHANGED\"",
        ProductDomainEventTypeRecord::DomainConditionChanged
    )]
    #[case(
        "\"DOMAIN_PROVENANCE_CHANGED\"",
        ProductDomainEventTypeRecord::DomainProvenanceChanged
    )]
    #[case(
        "\"DOMAIN_RESTORATION_CHANGED\"",
        ProductDomainEventTypeRecord::DomainRestorationChanged
    )]
    fn should_deserialize_product_event_type_record_in_screaming_snake_case(
        #[case] currency: &str,
        #[case] expected: ProductDomainEventTypeRecord,
    ) {
        let actual = serde_json::from_str::<ProductDomainEventTypeRecord>(currency).unwrap();
        assert_eq!(actual, expected);
    }
}
