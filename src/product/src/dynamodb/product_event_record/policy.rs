use crate::core::product_event::ProductPolicyEvent;
use crate::core::product_event::policy::{
    ProductPolicyEventPayload, ProhibitedContentProductPolicyEventPayload,
};
use crate::dynamodb::product_event_type_record::policy::ProductPolicyEventTypeRecord;
use crate::dynamodb::prohibited_content_record::{
    ProhibitedContentReasonRecord, ProhibitedContentRecord,
};
use common::event::Event;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::product_id::{ProductId, ProductKey};
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, error};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct ProductPolicyEventRecord {
    pub pk: String,
    pub sk: String,
    pub product_id: ProductId,
    pub event_id: EventId,
    pub event_type: ProductPolicyEventTypeRecord,
    pub event_type_schema_version: u8,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,

    pub prohibited_content_decision: ProhibitedContentRecord,
    pub prohibited_content_reason: ProhibitedContentReasonRecord,

    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

impl HasKey for ProductPolicyEventRecord {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        ProductKey {
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id.clone(),
        }
    }
}

pub fn mk_pk(shop_id: &ShopId, shops_product_id: &ShopsProductId) -> String {
    format!("product#shop_id#{shop_id}#shops_product_id#{shops_product_id}")
}

pub fn mk_sk(timestamp: &OffsetDateTime) -> Result<String, error::Format> {
    Ok(format!(
        "product#event#policy#{}",
        timestamp.format(&Rfc3339)?
    ))
}

impl TryFrom<ProductPolicyEvent> for ProductPolicyEventRecord {
    type Error = error::Format;

    fn try_from(event: ProductPolicyEvent) -> Result<Self, Self::Error> {
        let record = match event.payload {
            ProductPolicyEventPayload::ProhibitedContentDecision(payload) => {
                ProductPolicyEventRecord {
                    pk: mk_pk(&payload.shop_id, &payload.shops_product_id),
                    sk: mk_sk(&event.timestamp)?,
                    product_id: event.aggregate_id,
                    event_id: event.event_id,
                    event_type: ProductPolicyEventTypeRecord::PolicyProhibitedContentDecision,
                    event_type_schema_version: 0,
                    shop_id: payload.shop_id,
                    shops_product_id: payload.shops_product_id,
                    prohibited_content_decision: payload.decision.into(),
                    prohibited_content_reason: payload.reason.into(),
                    timestamp: event.timestamp,
                }
            }
        };

        Ok(record)
    }
}

impl From<ProductPolicyEventRecord> for ProductPolicyEvent {
    fn from(value: ProductPolicyEventRecord) -> Self {
        let prohibited_content_decision = ProhibitedContentProductPolicyEventPayload {
            shop_id: value.shop_id,
            shops_product_id: value.shops_product_id,
            decision: value.prohibited_content_decision.into(),
            reason: value.prohibited_content_reason.into(),
        };
        let payload =
            ProductPolicyEventPayload::ProhibitedContentDecision(prohibited_content_decision);

        Event {
            aggregate_id: value.product_id,
            event_id: value.event_id,
            timestamp: value.timestamp,
            payload,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for ProductPolicyEventRecord {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config
                .fake_with_rng::<ProductPolicyEvent, _>(rng)
                .try_into()
                .unwrap()
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::dynamodb::product_event_record::policy::ProductPolicyEventRecord;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_product_policy_event_record() {
            let _ = Faker.fake::<ProductPolicyEventRecord>();
        }
    }
}
