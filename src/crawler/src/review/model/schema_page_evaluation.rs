use super::SelectorFieldEvaluation;
use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaPageEvaluation {
    pub page_id: uuid::Uuid,
    pub url: String,
    pub role: String,
    pub apply_ok: bool,
    pub extracted: Option<RawExtractedProduct>,
    pub error: Option<String>,
    pub fields: Vec<SelectorFieldEvaluation>,
}
