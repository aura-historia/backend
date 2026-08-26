#![allow(dead_code)]

use product_listing_core::product_listing::{
    ListingSaleObservation, ProductListingAddress, ProductListingAuction,
    ProductListingEventPayload, ProductListingPricing,
};

use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_service::ports::product_listing_event_store::{
    ProductListingEvent, ProductListingEventStore, ProductListingEventStoreError,
    ProductListingEventStoreFactory,
};
use serde_json::{Value, json};
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingEventStoreFactory;

struct SqlxProductListingEventStore<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxProductListingEventStoreFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingEventStoreFactory<platform_postgres::SqlxTransaction>
    for SqlxProductListingEventStoreFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut platform_postgres::SqlxTransaction,
    ) -> impl ProductListingEventStore + 'tx {
        SqlxProductListingEventStore {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingEventStore for SqlxProductListingEventStore<'_> {
    async fn append(
        &mut self,
        event: &ProductListingEvent,
    ) -> Result<(), ProductListingEventStoreError> {
        sqlx::query(
            r#"
            INSERT INTO product_listing_events (
                event_id, product_listing_id, event_type, event_group, payload, event_time
            ) VALUES ($1, $2, $3, 'DOMAIN', $4, $5)
            "#,
        )
        .bind(uuid::Uuid::from(event.event_id))
        .bind(uuid::Uuid::from(event.aggregate_id))
        .bind(event.payload.event_type())
        .bind(event_payload_json(&event.payload))
        .bind(event.timestamp)
        .execute(&mut *self.connection)
        .await
        .map_err(ProductListingEventAppendSqlxError)?;

        Ok(())
    }
}

struct ProductListingEventAppendSqlxError(sqlx::Error);

impl From<ProductListingEventAppendSqlxError> for ProductListingEventStoreError {
    fn from(value: ProductListingEventAppendSqlxError) -> Self {
        let ProductListingEventAppendSqlxError(error) = value;
        match &error {
            sqlx::Error::Database(db_error) if db_error.is_unique_violation() => {
                Self::ProductListingEventAlreadyExists
            }
            _ => Self::ProductListingEventAppendFailed,
        }
    }
}

fn event_payload_json(payload: &ProductListingEventPayload) -> Value {
    match payload {
        ProductListingEventPayload::Created(payload) => json!({
            "kind": "created",
            "title": payload.title.as_ref().map(localized_title_json),
            "description": payload.description.as_ref().map(localized_description_json),
            "address": address_json(&payload.address),
            "pricing": pricing_json(payload.pricing),
            "availability": payload.availability.map(|value| value.as_str()),
            "url": payload.url.as_str(),
            "images": images_json(&payload.images),
            "auction": auction_json(payload.auction),
        }),
        ProductListingEventPayload::AvailabilityChanged(payload) => json!({
            "kind": "availabilityChanged",
            "previousAvailability": payload.previous.map(|value| value.as_str()),
            "currentAvailability": payload.current.map(|value| value.as_str()),
        }),
        ProductListingEventPayload::AddressChanged(payload) => json!({
            "kind": "addressChanged",
            "address": address_json(&payload.address),
        }),
        ProductListingEventPayload::PriceChanged(payload) => json!({
            "kind": "priceChanged",
            "oldPricing": pricing_json(payload.old_pricing),
            "newPricing": pricing_json(payload.new_pricing),
        }),
        ProductListingEventPayload::UrlChanged(payload) => json!({
            "kind": "urlChanged",
            "oldUrl": payload.old_url.as_str(),
            "newUrl": payload.new_url.as_str(),
        }),
        ProductListingEventPayload::ImagesChanged(payload) => json!({
            "kind": "imagesChanged",
            "images": images_json(&payload.images),
        }),
        ProductListingEventPayload::AuctionChanged(payload) => json!({
            "kind": "auctionChanged",
            "auction": auction_json(payload.auction),
        }),
        ProductListingEventPayload::Withdrawn(payload) => json!({
            "kind": "withdrawn",
            "previousAvailability": payload.previous_availability.map(|value| value.as_str()),
        }),
        ProductListingEventPayload::Restored(_) => json!({
            "kind": "restored",
        }),
        ProductListingEventPayload::SaleObserved(payload) => json!({
            "kind": "saleObserved",
            "observation": sale_observation_json(payload.observation),
        }),
        ProductListingEventPayload::SaleObservationRetracted(payload) => json!({
            "kind": "saleObservationRetracted",
            "observation": sale_observation_json(payload.observation),
        }),
    }
}

fn localized_title_json(
    title: &localization::Localized<localization::Language, product_listing_core::title::Title>,
) -> Value {
    json!({
        "language": title.localization.as_str(),
        "text": title.payload.as_ref(),
    })
}

fn localized_description_json(
    description: &localization::Localized<
        localization::Language,
        product_listing_core::description::Description,
    >,
) -> Value {
    json!({
        "language": description.localization.as_str(),
        "text": description.payload.as_ref(),
    })
}

fn address_json(address: &ProductListingAddress) -> Value {
    json!({
        "structured": address.structured.as_ref().map(|structured| json!({
            "addressline": structured.addressline,
            "addresslineExtra": structured.addressline_extra,
            "locality": structured.locality,
            "region": structured.region,
            "postalCode": structured.postal_code,
            "country": structured.country.map(|country| country.alpha3()),
        })),
        "geo": address.geo.map(|geo| json!({
            "lat": geo.lat,
            "lon": geo.lon,
        })),
    })
}

fn pricing_json(pricing: ProductListingPricing) -> Value {
    json!({
        "price": pricing.price.map(price_json),
        "priceEstimateMin": pricing.price_estimate_min.map(price_json),
        "priceEstimateMax": pricing.price_estimate_max.map(price_json),
    })
}

fn sale_observation_json(observation: ListingSaleObservation) -> Value {
    json!({
        "observedAt": observation.observed_at().to_string(),
        "fxRateId": observation.fx_rate_id().to_string(),
    })
}

fn price_json(price: money::Price) -> Value {
    json!({
        "amount": u64::from(price.monetary_amount),
        "currency": price.currency.as_str(),
    })
}

fn images_json(images: &indexmap::IndexSet<ProductListingImage>) -> Value {
    Value::Array(
        images
            .iter()
            .map(|image| {
                json!({
                    "url": image.url.as_str(),
                    "prohibitedContent": image.prohibited_content.as_str(),
                })
            })
            .collect(),
    )
}

fn auction_json(auction: ProductListingAuction) -> Value {
    json!({
        "start": auction.start.map(|value| value.to_string()),
        "end": auction.end.map(|value| value.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use product_listing_core::listing_availability::ListingAvailability;
    use product_listing_core::product_listing::{
        ListingAvailabilityChanged, ProductListingRestored, ProductListingWithdrawn,
    };

    #[test]
    fn should_write_canonical_nullable_availability() {
        let payload = ProductListingEventPayload::AvailabilityChanged(ListingAvailabilityChanged {
            previous: Some(ListingAvailability::InStock),
            current: None,
        });

        let json = event_payload_json(&payload);

        assert_eq!(
            Some("availabilityChanged"),
            json.get("kind").and_then(Value::as_str)
        );
        assert_eq!(
            Some("IN_STOCK"),
            json.get("previousAvailability").and_then(Value::as_str)
        );
        assert!(json.get("currentAvailability").is_some_and(Value::is_null));
    }

    #[test]
    fn should_write_lifecycle_payloads() {
        let withdrawn = event_payload_json(&ProductListingEventPayload::Withdrawn(
            ProductListingWithdrawn {
                previous_availability: Some(ListingAvailability::Available),
            },
        ));
        let restored = event_payload_json(&ProductListingEventPayload::Restored(
            ProductListingRestored,
        ));

        assert_eq!(
            Some("withdrawn"),
            withdrawn.get("kind").and_then(Value::as_str)
        );
        assert_eq!(
            Some("AVAILABLE"),
            withdrawn
                .get("previousAvailability")
                .and_then(Value::as_str)
        );
        assert_eq!(
            Some("restored"),
            restored.get("kind").and_then(Value::as_str)
        );
    }
}
