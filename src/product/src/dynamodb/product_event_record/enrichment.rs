use crate::core::product_event::ProductEnrichmentEvent;
use crate::core::product_event::enrichment::{
    EmbeddedTextProductEnrichmentEventPayload, ExtractedAttributesProductEnrichmentEventPayload,
    ProductEnrichmentEventPayload, TranslationProductEnrichmentEventPayload,
};
use crate::dynamodb::authenticity_record::AuthenticityRecord;
use crate::dynamodb::condition_record::ConditionRecord;
use crate::dynamodb::product_event_type_record::enrichment::ProductEnrichmentEventTypeRecord;
use crate::dynamodb::provenance_record::ProvenanceRecord;
use crate::dynamodb::restoration_record::RestorationRecord;
use common::error::missing_field::MissingPersistenceField;
use common::event::Event;
use common::event_id::EventId;
use common::language::record::LanguageRecord;
use common::product_id::ProductId;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::year::Year;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, error};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct ProductEnrichmentEventRecord {
    pub pk: String,
    pub sk: String,
    pub product_id: ProductId,
    pub event_id: EventId,
    pub event_type: ProductEnrichmentEventTypeRecord,
    pub event_type_schema_version: u8,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,

    // translation
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_language: Option<LanguageRecord>,
    pub target_language: Option<LanguageRecord>,
    pub target: Option<String>,

    // text-embedding
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub text_embedding: Option<Vec<f32>>,

    // attribute-extraction
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_year_min: Option<Year>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_year: Option<Year>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_year_max: Option<Year>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub authenticity: Option<AuthenticityRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub condition: Option<ConditionRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub provenance: Option<ProvenanceRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub restoration: Option<RestorationRecord>,

    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

pub fn mk_pk(shop_id: &ShopId, shops_product_id: &ShopsProductId) -> String {
    format!("product#shop_id#{shop_id}#shops_product_id#{shops_product_id}")
}

pub fn mk_sk(timestamp: &OffsetDateTime) -> Result<String, error::Format> {
    Ok(format!(
        "product#event#enrichment#{}",
        timestamp.format(&Rfc3339)?
    ))
}

impl TryFrom<ProductEnrichmentEvent> for ProductEnrichmentEventRecord {
    type Error = error::Format;

    fn try_from(event: ProductEnrichmentEvent) -> Result<Self, Self::Error> {
        let record = match event.payload {
            ProductEnrichmentEventPayload::TranslatedTitle(payload) => {
                ProductEnrichmentEventRecord {
                    pk: mk_pk(&payload.shop_id, &payload.shops_product_id),
                    sk: mk_sk(&event.timestamp)?,
                    product_id: event.aggregate_id,
                    event_id: event.event_id,
                    event_type: ProductEnrichmentEventTypeRecord::EnrichmentTranslatedTitle,
                    event_type_schema_version: 0,
                    shop_id: payload.shop_id,
                    shops_product_id: payload.shops_product_id,
                    source_language: Some(payload.source_language.into()),
                    target_language: Some(payload.target_language.into()),
                    target: Some(payload.target.into()),
                    text_embedding: None,
                    origin_year_min: None,
                    origin_year: None,
                    origin_year_max: None,
                    authenticity: None,
                    condition: None,
                    provenance: None,
                    restoration: None,
                    timestamp: event.timestamp,
                }
            }
            ProductEnrichmentEventPayload::TranslatedDescription(payload) => {
                ProductEnrichmentEventRecord {
                    pk: mk_pk(&payload.shop_id, &payload.shops_product_id),
                    sk: mk_sk(&event.timestamp)?,
                    product_id: event.aggregate_id,
                    event_id: event.event_id,
                    event_type: ProductEnrichmentEventTypeRecord::EnrichmentTranslatedDescription,
                    event_type_schema_version: 0,
                    shop_id: payload.shop_id,
                    shops_product_id: payload.shops_product_id,
                    source_language: Some(payload.source_language.into()),
                    target_language: Some(payload.target_language.into()),
                    target: Some(payload.target.into()),
                    text_embedding: None,
                    origin_year_min: None,
                    origin_year: None,
                    origin_year_max: None,
                    authenticity: None,
                    condition: None,
                    provenance: None,
                    restoration: None,
                    timestamp: event.timestamp,
                }
            }
            ProductEnrichmentEventPayload::EmbeddedText(payload) => ProductEnrichmentEventRecord {
                pk: mk_pk(&payload.shop_id, &payload.shops_product_id),
                sk: mk_sk(&event.timestamp)?,
                product_id: event.aggregate_id,
                event_id: event.event_id,
                event_type: ProductEnrichmentEventTypeRecord::EnrichmentEmbeddedText,
                event_type_schema_version: 0,
                shop_id: payload.shop_id,
                shops_product_id: payload.shops_product_id,
                source_language: None,
                target_language: None,
                target: None,
                text_embedding: Some(payload.embedding),
                origin_year_min: None,
                origin_year: None,
                origin_year_max: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
                timestamp: event.timestamp,
            },
            ProductEnrichmentEventPayload::ExtractedAttributes(payload) => {
                ProductEnrichmentEventRecord {
                    pk: mk_pk(&payload.shop_id, &payload.shops_product_id),
                    sk: mk_sk(&event.timestamp)?,
                    product_id: event.aggregate_id,
                    event_id: event.event_id,
                    event_type: ProductEnrichmentEventTypeRecord::EnrichmentExtractedAttributes,
                    event_type_schema_version: 0,
                    shop_id: payload.shop_id,
                    shops_product_id: payload.shops_product_id,
                    source_language: None,
                    target_language: None,
                    target: None,
                    text_embedding: None,
                    origin_year_min: payload.origin_year_min,
                    origin_year: payload.origin_year,
                    origin_year_max: payload.origin_year_max,
                    authenticity: payload.authenticity.map(Into::into),
                    condition: payload.condition.map(Into::into),
                    provenance: payload.provenance.map(Into::into),
                    restoration: payload.restoration.map(Into::into),
                    timestamp: event.timestamp,
                }
            }
        };

        Ok(record)
    }
}

impl TryFrom<ProductEnrichmentEventRecord> for ProductEnrichmentEvent {
    type Error = MissingPersistenceField;

    fn try_from(record: ProductEnrichmentEventRecord) -> Result<Self, Self::Error> {
        match record.event_type {
            ProductEnrichmentEventTypeRecord::EnrichmentTranslatedTitle => {
                let event = Event {
                    aggregate_id: record.product_id,
                    event_id: record.event_id,
                    timestamp: record.timestamp,
                    payload: ProductEnrichmentEventPayload::TranslatedTitle(
                        TranslationProductEnrichmentEventPayload {
                            shop_id: record.shop_id,
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
                };
                Ok(event)
            }
            ProductEnrichmentEventTypeRecord::EnrichmentTranslatedDescription => {
                let event = Event {
                    aggregate_id: record.product_id,
                    event_id: record.event_id,
                    timestamp: record.timestamp,
                    payload: ProductEnrichmentEventPayload::TranslatedDescription(
                        TranslationProductEnrichmentEventPayload {
                            shop_id: record.shop_id,
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
                };
                Ok(event)
            }
            ProductEnrichmentEventTypeRecord::EnrichmentEmbeddedText => {
                let event = Event {
                    aggregate_id: record.product_id,
                    event_id: record.event_id,
                    timestamp: record.timestamp,
                    payload: ProductEnrichmentEventPayload::EmbeddedText(
                        EmbeddedTextProductEnrichmentEventPayload {
                            shop_id: record.shop_id,
                            shops_product_id: record.shops_product_id,
                            embedding: record.text_embedding.ok_or(
                                MissingPersistenceField::new(
                                    field::field!(text_embedding@ProductEnrichmentEventRecord),
                                ),
                            )?,
                        },
                    ),
                };
                Ok(event)
            }
            ProductEnrichmentEventTypeRecord::EnrichmentExtractedAttributes => {
                let event = Event {
                    aggregate_id: record.product_id,
                    event_id: record.event_id,
                    timestamp: record.timestamp,
                    payload: ProductEnrichmentEventPayload::ExtractedAttributes(
                        ExtractedAttributesProductEnrichmentEventPayload {
                            shop_id: record.shop_id,
                            shops_product_id: record.shops_product_id,
                            origin_year_min: record.origin_year_min,
                            origin_year: record.origin_year,
                            origin_year_max: record.origin_year_max,
                            authenticity: record.authenticity.map(Into::into),
                            condition: record.condition.map(Into::into),
                            provenance: record.provenance.map(Into::into),
                            restoration: record.restoration.map(Into::into),
                        },
                    ),
                };
                Ok(event)
            }
        }
    }
}
