use crate::dynamodb::product_event_type_record::policy::ProductPolicyEventTypeRecord;
use crate::dynamodb::prohibited_content_record::ProhibitedContentRecord;
use common::event_id::EventId;
use common::product_id::ProductId;
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
    pub prohibited_content_reason: String,

    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

pub fn mk_pk(shop_id: &ShopId, shops_product_id: &ShopsProductId) -> String {
    format!("product#shop_id#{shop_id}#shops_product_id#{shops_product_id}")
}

pub fn mk_sk(timestamp: &OffsetDateTime) -> Result<String, error::Format> {
    Ok(format!(
        "product#event#policy{}",
        timestamp.format(&Rfc3339)?
    ))
}
