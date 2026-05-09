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
        }
    }
}
