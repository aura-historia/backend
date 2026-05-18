use crate::core::product_event::ProductEnrichmentEvent;
use crate::core::product_event::enrichment::{
    EmbeddedProductEnrichmentEventPayload, ProductEnrichmentEventPayload,
    TranslationProductEnrichmentEventPayload,
};
use crate::dynamodb::product_event_type_record::enrichment::ProductEnrichmentEventTypeRecord;
use common::error::missing_field::MissingPersistenceField;
use common::event::Event;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::language::record::LanguageRecord;
use common::product_id::{ProductId, ProductKey};
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct ProductEnrichmentEventRecord {
    pub pk: String,
    pub sk: String,
    pub product_id: ProductId,
    pub event_id: EventId,
    pub event_type: ProductEnrichmentEventTypeRecord,
    pub event_type_schema_version: u8,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_language: Option<LanguageRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub target_language: Option<LanguageRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub embedding: Option<Vec<f32>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub native_title: Option<String>,
    /// Language of [`Self::native_title`].  Present on `ENRICHMENT_EMBEDDED` records
    /// when `native_title` is also present, so downstream pipeline stages (e.g. the
    /// translation lambda) can determine the source language without an additional lookup.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub native_title_language: Option<LanguageRecord>,
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

impl HasKey for ProductEnrichmentEventRecord {
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

pub fn mk_sk(event_id: &EventId) -> String {
    format!("product#event#enrichment#{event_id}")
}

impl From<ProductEnrichmentEvent> for ProductEnrichmentEventRecord {
    fn from(event: ProductEnrichmentEvent) -> Self {
        match event.payload {
            ProductEnrichmentEventPayload::TranslatedTitle(payload) => {
                ProductEnrichmentEventRecord {
                    pk: mk_pk(&payload.shop_id, &payload.shops_product_id),
                    sk: mk_sk(&event.event_id),
                    product_id: event.aggregate_id,
                    event_id: event.event_id,
                    event_type: ProductEnrichmentEventTypeRecord::EnrichmentTranslatedTitle,
                    event_type_schema_version: 0,
                    shop_id: payload.shop_id,
                    seller_id: payload.seller_id,
                    shops_product_id: payload.shops_product_id,
                    source_language: Some(payload.source_language.into()),
                    target_language: Some(payload.target_language.into()),
                    target: Some(payload.target.into()),
                    embedding: None,
                    native_title: None,
                    native_title_language: None,
                    timestamp: event.timestamp,
                }
            }
            ProductEnrichmentEventPayload::Embedded(payload) => ProductEnrichmentEventRecord {
                pk: mk_pk(&payload.shop_id, &payload.shops_product_id),
                sk: mk_sk(&event.event_id),
                product_id: event.aggregate_id,
                event_id: event.event_id,
                event_type: ProductEnrichmentEventTypeRecord::EnrichmentEmbedded,
                event_type_schema_version: 0,
                shop_id: payload.shop_id,
                seller_id: payload.seller_id,
                shops_product_id: payload.shops_product_id,
                source_language: None,
                target_language: None,
                target: None,
                embedding: Some(payload.embedding),
                native_title: payload.native_title.map(Into::into),
                native_title_language: payload.native_title_language.map(Into::into),
                timestamp: event.timestamp,
            },
        }
    }
}

impl TryFrom<ProductEnrichmentEventRecord> for ProductEnrichmentEvent {
    type Error = MissingPersistenceField;

    fn try_from(record: ProductEnrichmentEventRecord) -> Result<Self, Self::Error> {
        match record.event_type {
            ProductEnrichmentEventTypeRecord::EnrichmentTranslatedTitle => Ok(Event {
                aggregate_id: record.product_id,
                event_id: record.event_id,
                timestamp: record.timestamp,
                payload: ProductEnrichmentEventPayload::TranslatedTitle(
                    TranslationProductEnrichmentEventPayload {
                        shop_id: record.shop_id,
                        seller_id: record.seller_id,
                        shops_product_id: record.shops_product_id,
                        source_language: record
                            .source_language
                            .ok_or(MissingPersistenceField::new(
                                field::field!(source_language@ProductEnrichmentEventRecord),
                            ))?
                            .into(),
                        target_language: record
                            .target_language
                            .ok_or(MissingPersistenceField::new(
                                field::field!(target_language@ProductEnrichmentEventRecord),
                            ))?
                            .into(),
                        target: record
                            .target
                            .ok_or(MissingPersistenceField::new(
                                field::field!(target@ProductEnrichmentEventRecord),
                            ))?
                            .into(),
                    },
                ),
            }),
            ProductEnrichmentEventTypeRecord::EnrichmentEmbedded => Ok(Event {
                aggregate_id: record.product_id,
                event_id: record.event_id,
                timestamp: record.timestamp,
                payload: ProductEnrichmentEventPayload::Embedded(
                    EmbeddedProductEnrichmentEventPayload {
                        shop_id: record.shop_id,
                        seller_id: record.seller_id,
                        shops_product_id: record.shops_product_id,
                        embedding: record.embedding.ok_or(MissingPersistenceField::new(
                            field::field!(embedding@ProductEnrichmentEventRecord),
                        ))?,
                        native_title: record.native_title.map(Into::into),
                        native_title_language: record.native_title_language.map(Into::into),
                    },
                ),
            }),
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ProductEnrichmentEventRecord {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config
                .fake_with_rng::<ProductEnrichmentEvent, _>(rng)
                .into()
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::dynamodb::product_event_record::enrichment::ProductEnrichmentEventRecord;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_product_enrichment_event_record() {
            let _ = Faker.fake::<ProductEnrichmentEventRecord>();
        }
    }
}
