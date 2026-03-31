use crate::{
    core::product_event::{ProductEvent, ProductEventPayload},
    dynamodb::product_event_record::{
        domain::ProductDomainEventRecord, enrichment::ProductEnrichmentEventRecord,
        policy::ProductPolicyEventRecord,
    },
};
use common::{
    event::Event,
    event_id::EventId,
    has_key::HasKey,
    product_id::{ProductId, ProductKey},
};
use serde::{Deserialize, Serialize};

pub mod domain;
pub mod enrichment;
pub mod policy;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)] // fine as related Product[TYPE]EventTypeRecord disjoint - their contructors are globally unique
#[allow(clippy::large_enum_variant)]
pub enum ProductEventRecord {
    Domain(ProductDomainEventRecord),
    Enrichment(ProductEnrichmentEventRecord),
    Policy(ProductPolicyEventRecord),
}

impl From<ProductDomainEventRecord> for ProductEventRecord {
    fn from(domain_record: ProductDomainEventRecord) -> Self {
        Self::Domain(domain_record)
    }
}

impl ProductEventRecord {
    pub fn product_id(&self) -> &ProductId {
        match self {
            ProductEventRecord::Domain(record) => &record.product_id,
            ProductEventRecord::Enrichment(record) => &record.product_id,
            ProductEventRecord::Policy(record) => &record.product_id,
        }
    }

    pub fn event_id(&self) -> &EventId {
        match self {
            ProductEventRecord::Domain(record) => &record.event_id,
            ProductEventRecord::Enrichment(record) => &record.event_id,
            ProductEventRecord::Policy(record) => &record.event_id,
        }
    }
}

impl HasKey for ProductEventRecord {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        match self {
            ProductEventRecord::Domain(record) => record.key(),
            ProductEventRecord::Enrichment(record) => record.key(),
            ProductEventRecord::Policy(record) => record.key(),
        }
    }
}

impl From<ProductEnrichmentEventRecord> for ProductEventRecord {
    fn from(enrichment_record: ProductEnrichmentEventRecord) -> Self {
        Self::Enrichment(enrichment_record)
    }
}

impl From<ProductPolicyEventRecord> for ProductEventRecord {
    fn from(policy_record: ProductPolicyEventRecord) -> Self {
        Self::Policy(policy_record)
    }
}

impl From<ProductEvent> for ProductEventRecord {
    fn from(event: ProductEvent) -> Self {
        match event.payload {
            ProductEventPayload::ProductDomainEvent(payload) => {
                let helper = Event {
                    aggregate_id: event.aggregate_id,
                    event_id: event.event_id,
                    timestamp: event.timestamp,
                    payload,
                };
                let helper_record = helper.into();
                ProductEventRecord::Domain(helper_record)
            }
            ProductEventPayload::ProductEnrichmentEvent(payload) => {
                let helper = Event {
                    aggregate_id: event.aggregate_id,
                    event_id: event.event_id,
                    timestamp: event.timestamp,
                    payload,
                };
                let helper_record = helper.into();
                ProductEventRecord::Enrichment(helper_record)
            }
            ProductEventPayload::ProductPolicyEvent(payload) => {
                let helper = Event {
                    aggregate_id: event.aggregate_id,
                    event_id: event.event_id,
                    timestamp: event.timestamp,
                    payload,
                };
                let helper_record = helper.into();
                ProductEventRecord::Policy(helper_record)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::dynamodb::product_event_record::{
        ProductEventRecord, domain::ProductDomainEventRecord,
        enrichment::ProductEnrichmentEventRecord, policy::ProductPolicyEventRecord,
    };
    use fake::{Fake, Faker};

    #[rstest::rstest]
    #[case(Faker.fake::<ProductDomainEventRecord>())]
    #[case(Faker.fake::<ProductEnrichmentEventRecord>())]
    #[case(Faker.fake::<ProductPolicyEventRecord>())]
    fn should_serialize_flat_product_event_record(
        #[case] event_record: impl Into<ProductEventRecord>,
    ) {
        let product_event_record = event_record.into();

        let actual = serde_json::to_value(&product_event_record).unwrap();

        assert!(actual["pk"].as_str().unwrap().starts_with("product#"));
        assert!(actual["sk"].as_str().unwrap().starts_with("product#event#"));
    }

    #[rstest::rstest]
    #[case(Faker.fake::<ProductDomainEventRecord>())]
    #[case(Faker.fake::<ProductEnrichmentEventRecord>())]
    #[case(Faker.fake::<ProductPolicyEventRecord>())]
    fn should_include_event_id_in_sort_key_when_converting_to_product_event_record(
        #[case] event_record: impl Into<ProductEventRecord>,
    ) {
        let product_event_record = event_record.into();

        let (sk, event_id) = match &product_event_record {
            ProductEventRecord::Domain(record) => (&record.sk, record.event_id),
            ProductEventRecord::Enrichment(record) => (&record.sk, record.event_id),
            ProductEventRecord::Policy(record) => (&record.sk, record.event_id),
        };

        assert!(sk.ends_with(&event_id.to_string()));
    }
}
