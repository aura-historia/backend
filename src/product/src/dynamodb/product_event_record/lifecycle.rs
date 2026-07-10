use crate::core::product_event::ProductLifecycleEvent;
use crate::core::product_event::lifecycle::{
    ProductCommonLifecycleEventPayload, ProductLifecycleEventPayload,
};
use crate::dynamodb::product_event_type_record::lifecycle::ProductLifecycleEventTypeRecord;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::product_id::{ProductId, ProductKey};
use common::product_lifecycle::record::ProductLifecycleRecord;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use time::OffsetDateTime;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct ProductLifecycleEventRecord {
    pub pk: String,
    pub sk: String,
    pub product_id: ProductId,
    pub event_id: EventId,
    pub event_type: ProductLifecycleEventTypeRecord,
    pub event_type_schema_version: u8,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub new_lifecycle: ProductLifecycleRecord,
    pub old_lifecycle: ProductLifecycleRecord,
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

pub fn mk_pk(shop_id: &ShopId, shops_product_id: &ShopsProductId) -> String {
    format!("product#shop_id#{shop_id}#shops_product_id#{shops_product_id}")
}

pub fn mk_sk(event_id: &EventId) -> String {
    format!("product#event#lifecycle#{event_id}")
}

impl HasKey for ProductLifecycleEventRecord {
    type Key = ProductKey;

    fn key(&self) -> ProductKey {
        ProductKey::new(self.shop_id, self.shops_product_id.clone())
    }
}

impl From<ProductLifecycleEvent> for ProductLifecycleEventRecord {
    fn from(event: ProductLifecycleEvent) -> Self {
        let shop_id = *event.payload.shop_id();
        let shops_product_id = event.payload.shops_product_id().clone();
        let event_type = (&event.payload).into();
        match event.payload {
            ProductLifecycleEventPayload::Deleted(payload) => ProductLifecycleEventRecord {
                pk: mk_pk(&shop_id, &shops_product_id),
                sk: mk_sk(&event.event_id),
                product_id: event.aggregate_id,
                event_id: event.event_id,
                event_type,
                event_type_schema_version: 0,
                shop_id,
                seller_id: payload.seller_id,
                shops_product_id,
                new_lifecycle: payload.new_lifecycle.into(),
                old_lifecycle: payload.old_lifecycle.into(),
                timestamp: event.timestamp,
            },
        }
    }
}
