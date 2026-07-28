#![allow(dead_code)]

use crate::core::product_aggregate::{ProductDomainEvent, ProductDomainEventPayload};
use crate::service::ports::product_event_store::{ProductEventStore, ProductEventStoreError};
use common::event_id::EventId;
use common::product_id::ProductId;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
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
    async fn append(
        &mut self,
        event: &ProductDomainEvent,
        created_by: &str,
    ) -> Result<(), ProductEventStoreError> {
        let (shop_id, shops_product_id) = event_key(event);
        sqlx::query(
            r#"
            INSERT INTO product_events (
                event_id, product_id, shop_id, shops_product_id, event_type,
                event_group, payload, event_time, created_by
            ) VALUES ($1, $2, $3, $4, $5, 'DOMAIN', $6, $7, $8)
            "#,
        )
        .bind(uuid::Uuid::from(event.event_id))
        .bind(uuid::Uuid::from(event.aggregate_id))
        .bind(uuid::Uuid::from(shop_id))
        .bind(shops_product_id.as_ref())
        .bind(event.payload.event_type())
        .bind(event_payload_json(&event.payload))
        .bind(event.timestamp)
        .bind(created_by)
        .execute(&mut *self.connection)
        .await
        .map_err(map_sqlx_error)?;

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
        .await
        .map_err(map_sqlx_error)?;

        Ok(event_id.map(EventId::from))
    }
}

fn event_key(event: &ProductDomainEvent) -> (ShopId, ShopsProductId) {
    match &event.payload {
        ProductDomainEventPayload::Created(payload) => {
            (payload.shop_id, payload.shops_product_id.clone())
        }
        ProductDomainEventPayload::StateChanged(payload) => {
            (payload.shop_id, payload.shops_product_id.clone())
        }
        ProductDomainEventPayload::AddressChanged(payload) => {
            (payload.shop_id, payload.shops_product_id.clone())
        }
        ProductDomainEventPayload::PriceChanged(payload) => {
            (payload.shop_id, payload.shops_product_id.clone())
        }
        ProductDomainEventPayload::UrlChanged(payload) => {
            (payload.shop_id, payload.shops_product_id.clone())
        }
        ProductDomainEventPayload::ImagesChanged(payload) => {
            (payload.shop_id, payload.shops_product_id.clone())
        }
        ProductDomainEventPayload::Embedded(payload) => {
            (payload.shop_id, payload.shops_product_id.clone())
        }
        ProductDomainEventPayload::AuctionChanged(payload) => {
            (payload.shop_id, payload.shops_product_id.clone())
        }
        ProductDomainEventPayload::Deleted(payload) => {
            (payload.shop_id, payload.shops_product_id.clone())
        }
    }
}

fn event_payload_json(payload: &ProductDomainEventPayload) -> serde_json::Value {
    match payload {
        ProductDomainEventPayload::Created(payload) => json!({
            "kind": "created",
            "shopId": payload.shop_id.to_string(),
            "sellerId": payload.seller_id.to_string(),
            "shopsProductId": payload.shops_product_id.as_ref(),
            "state": format!("{:?}", payload.state),
        }),
        ProductDomainEventPayload::StateChanged(payload) => json!({
            "kind": "stateChanged",
            "shopId": payload.shop_id.to_string(),
            "shopsProductId": payload.shops_product_id.as_ref(),
            "oldState": format!("{:?}", payload.old_state),
            "newState": format!("{:?}", payload.new_state),
        }),
        ProductDomainEventPayload::AddressChanged(payload) => json!({
            "kind": "addressChanged",
            "shopId": payload.shop_id.to_string(),
            "shopsProductId": payload.shops_product_id.as_ref(),
            "hasStructuredAddress": payload.address.structured.is_some(),
            "hasGeoAddress": payload.address.geo.is_some(),
        }),
        ProductDomainEventPayload::PriceChanged(payload) => json!({
            "kind": "priceChanged",
            "shopId": payload.shop_id.to_string(),
            "shopsProductId": payload.shops_product_id.as_ref(),
            "oldFxRateId": payload.old_pricing.fx_rate_id.map(String::from),
            "newFxRateId": payload.new_pricing.fx_rate_id.map(String::from),
        }),
        ProductDomainEventPayload::UrlChanged(payload) => json!({
            "kind": "urlChanged",
            "shopId": payload.shop_id.to_string(),
            "shopsProductId": payload.shops_product_id.as_ref(),
            "oldUrl": payload.old_url.as_str(),
            "newUrl": payload.new_url.as_str(),
        }),
        ProductDomainEventPayload::ImagesChanged(payload) => json!({
            "kind": "imagesChanged",
            "shopId": payload.shop_id.to_string(),
            "shopsProductId": payload.shops_product_id.as_ref(),
            "imageCount": payload.images.len(),
        }),
        ProductDomainEventPayload::Embedded(payload) => json!({
            "kind": "embedded",
            "shopId": payload.shop_id.to_string(),
            "shopsProductId": payload.shops_product_id.as_ref(),
            "dimensions": payload.embedding.as_ref().map(|embedding| embedding.len()),
        }),
        ProductDomainEventPayload::AuctionChanged(payload) => json!({
            "kind": "auctionChanged",
            "shopId": payload.shop_id.to_string(),
            "shopsProductId": payload.shops_product_id.as_ref(),
            "auctionStart": payload.auction.start.map(|value| value.to_string()),
            "auctionEnd": payload.auction.end.map(|value| value.to_string()),
        }),
        ProductDomainEventPayload::Deleted(payload) => json!({
            "kind": "deleted",
            "shopId": payload.shop_id.to_string(),
            "shopsProductId": payload.shops_product_id.as_ref(),
            "oldLifecycle": format!("{:?}", payload.old_lifecycle),
            "newLifecycle": format!("{:?}", payload.new_lifecycle),
        }),
    }
}

fn map_sqlx_error(error: sqlx::Error) -> ProductEventStoreError {
    match &error {
        sqlx::Error::Database(db_error) if db_error.is_unique_violation() => {
            ProductEventStoreError::EventConflict
        }
        _ => ProductEventStoreError::Internal,
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
                shop_id: ShopId::new(),
                shops_product_id: ShopsProductId::new(),
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
                shop_id: ShopId::new(),
                shops_product_id: ShopsProductId::new(),
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
