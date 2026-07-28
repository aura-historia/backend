#![allow(dead_code)]

use crate::core::product_aggregate::{ProductDomainEvent, ProductDomainEventPayload};
use crate::service::ports::product_event_store::{ProductEventStore, ProductEventStoreError};
use common::event_id::EventId;
use common::product_id::ProductId;
use serde_json::json;
use sqlx::PgConnection;

pub(crate) struct SqlxProductEventStore<'tx> {
    connection: &'tx mut PgConnection,
}

impl<'tx> SqlxProductEventStore<'tx> {
    pub(crate) fn new(connection: &'tx mut PgConnection) -> Self {
        Self { connection }
    }
}

#[async_trait::async_trait]
impl ProductEventStore for SqlxProductEventStore<'_> {
    async fn append(&mut self, event: &ProductDomainEvent) -> Result<(), ProductEventStoreError> {
        sqlx::query(
            r#"
            INSERT INTO product_events (
                event_id, product_id, event_type, event_group, payload, event_time
            ) VALUES ($1, $2, $3, 'DOMAIN', $4, $5)
            "#,
        )
        .bind(uuid::Uuid::from(event.event_id))
        .bind(uuid::Uuid::from(event.aggregate_id))
        .bind(event.payload.event_type())
        .bind(event_payload_json(&event.payload))
        .bind(event.timestamp)
        .execute(&mut *self.connection)
        .await?;

        Ok(())
    }

    async fn find_current_event_id(
        &mut self,
        product_id: ProductId,
    ) -> Result<Option<EventId>, ProductEventStoreError> {
        let event_id = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT event_id FROM products WHERE product_id = $1",
        )
        .bind(uuid::Uuid::from(product_id))
        .fetch_optional(&mut *self.connection)
        .await?;

        Ok(event_id.map(EventId::from))
    }
}

fn event_payload_json(payload: &ProductDomainEventPayload) -> serde_json::Value {
    match payload {
        ProductDomainEventPayload::Created(payload) => json!({
            "kind": "created",
            "state": format!("{:?}", payload.state),
            "hasTitle": payload.title.is_some(),
            "hasDescription": payload.description.is_some(),
        }),
        ProductDomainEventPayload::StateChanged(payload) => json!({
            "kind": "stateChanged",
            "oldState": format!("{:?}", payload.old_state),
            "newState": format!("{:?}", payload.new_state),
        }),
        ProductDomainEventPayload::AddressChanged(payload) => json!({
            "kind": "addressChanged",
            "hasStructuredAddress": payload.address.structured.is_some(),
            "hasGeoAddress": payload.address.geo.is_some(),
        }),
        ProductDomainEventPayload::PriceChanged(payload) => json!({
            "kind": "priceChanged",
            "oldFxRateId": payload.old_pricing.fx_rate_id.map(String::from),
            "newFxRateId": payload.new_pricing.fx_rate_id.map(String::from),
        }),
        ProductDomainEventPayload::UrlChanged(payload) => json!({
            "kind": "urlChanged",
            "oldUrl": payload.old_url.as_str(),
            "newUrl": payload.new_url.as_str(),
        }),
        ProductDomainEventPayload::ImagesChanged(payload) => json!({
            "kind": "imagesChanged",
            "imageCount": payload.images.len(),
        }),
        ProductDomainEventPayload::AuctionChanged(payload) => json!({
            "kind": "auctionChanged",
            "auctionStart": payload.auction.start.map(|value| value.to_string()),
            "auctionEnd": payload.auction.end.map(|value| value.to_string()),
        }),
        ProductDomainEventPayload::Deleted(payload) => json!({
            "kind": "deleted",
            "oldLifecycle": format!("{:?}", payload.old_lifecycle),
            "newLifecycle": format!("{:?}", payload.new_lifecycle),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::product_aggregate::{ProductPricing, ProductStateChanged};
    use common::event::Event;
    use common::product_state::domain::ProductState;
    use time::OffsetDateTime;

    #[test]
    fn should_map_state_changed_event_to_payload_object() {
        let event = Event {
            aggregate_id: ProductId::new(),
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::StateChanged(ProductStateChanged {
                old_state: ProductState::Listed,
                new_state: ProductState::Available,
            }),
        };

        let payload = event_payload_json(&event.payload);

        assert_eq!(
            Some("stateChanged"),
            payload.get("kind").and_then(|value| value.as_str())
        );
    }

    #[test]
    fn should_map_price_event_fx_rate_ids() {
        let old_pricing = ProductPricing::default();
        let new_pricing = ProductPricing::default();

        let payload = event_payload_json(&ProductDomainEventPayload::PriceChanged(
            crate::core::product_aggregate::ProductPriceChanged {
                old_pricing,
                new_pricing,
            },
        ));

        assert_eq!(
            Some("priceChanged"),
            payload.get("kind").and_then(|value| value.as_str())
        );
    }
}
