use crate::dynamodb::authenticity_record::AuthenticityRecord;
use crate::dynamodb::condition_record::ConditionRecord;
use crate::dynamodb::product_event_type_record::domain::ProductDomainEventTypeRecord;
use crate::dynamodb::provenance_record::ProvenanceRecord;
use crate::dynamodb::restoration_record::RestorationRecord;
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
pub struct ProductDomainEventRecord {
    pub pk: String,
    pub sk: String,
    pub product_id: ProductId,
    pub event_id: EventId,
    pub event_type: ProductDomainEventTypeRecord,
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
