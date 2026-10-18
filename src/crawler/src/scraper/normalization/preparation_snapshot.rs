//! Exact crawler-preparation evidence for the `CRAWLER_PREPARATION_V1` replay entry point.
//!
//! Raw capture cannot reproduce a candidate discarded during preparation, so the snapshot keeps
//! the complete extracted product handed to preparation (including the selected images), the
//! effective URL, the original source-record key and the fallback currency.

use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use crate::scraper::normalization::product_normalization_service::{
    CrawlerPreparationEvaluation, ProductNormalizationContext, evaluate_preparation,
};
use money::Currency;
use product_listing_normalization::{
    MAX_SOURCE_PAYLOAD_JSON_BYTES, NormalizationInputError, canonical_json_sha256,
};
use serde::{Deserialize, Serialize};
use url::Url;

/// Snapshots above this size are logged but not retained as replay evidence.
pub const MAX_PREPARATION_SNAPSHOT_JSON_BYTES: usize = MAX_SOURCE_PAYLOAD_JSON_BYTES;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CrawlerPreparationSnapshot {
    pub extracted_product: RawExtractedProduct,
    pub effective_url: String,
    pub source_record_key: String,
    pub fallback_currency: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CrawlerPreparationSnapshotError {
    #[error("crawler preparation snapshot does not match the V1 contract")]
    Contract(#[source] serde_json::Error),
    #[error("crawler preparation snapshot effective URL is invalid")]
    EffectiveUrl(#[source] url::ParseError),
    #[error("crawler preparation snapshot fallback currency is unsupported")]
    FallbackCurrency,
}

impl CrawlerPreparationSnapshot {
    pub fn new(raw: &RawExtractedProduct, context: &ProductNormalizationContext) -> Self {
        Self {
            extracted_product: raw.clone(),
            effective_url: context.effective_url.to_string(),
            source_record_key: context.source_record_key.clone(),
            fallback_currency: context
                .fallback_currency
                .map(|currency| currency.as_str().to_owned()),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    pub fn from_json(value: serde_json::Value) -> Result<Self, CrawlerPreparationSnapshotError> {
        serde_json::from_value(value).map_err(CrawlerPreparationSnapshotError::Contract)
    }

    /// Key-order-stable identity of the exact preparation input.
    pub fn input_sha256(&self) -> Result<[u8; 32], NormalizationInputError> {
        canonical_json_sha256(&self.to_json())
    }

    /// Calls the production preparation function on the complete saved input.
    pub fn evaluate(
        &self,
    ) -> Result<CrawlerPreparationEvaluation, CrawlerPreparationSnapshotError> {
        let effective_url = Url::parse(&self.effective_url)
            .map_err(CrawlerPreparationSnapshotError::EffectiveUrl)?;
        let fallback_currency = self
            .fallback_currency
            .as_deref()
            .map(|code| {
                Currency::from_code(code).ok_or(CrawlerPreparationSnapshotError::FallbackCurrency)
            })
            .transpose()?;
        Ok(evaluate_preparation(
            self.extracted_product.clone(),
            ProductNormalizationContext {
                effective_url,
                source_record_key: self.source_record_key.clone(),
                fallback_currency,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn snapshots_round_trip_and_replay_the_production_preparation() {
        let raw = RawExtractedProduct {
            source_listing_id: "listing-1".to_owned(),
            title: "   ".to_owned(),
            description: vec![],
            price: Some("-100 EUR".to_owned()),
            price_estimate_min: None,
            price_estimate_max: None,
            state: "in stock".to_owned(),
            images: vec![],
            raw_attributes: BTreeMap::new(),
        };
        let context = ProductNormalizationContext {
            effective_url: Url::parse("https://example.com/item/1").unwrap(),
            source_record_key: "https://example.com/item/1".to_owned(),
            fallback_currency: Some(Currency::Eur),
        };
        let snapshot = CrawlerPreparationSnapshot::new(&raw, &context);
        let decoded = CrawlerPreparationSnapshot::from_json(snapshot.to_json()).unwrap();
        assert_eq!(snapshot, decoded);
        assert_eq!(
            snapshot.input_sha256().unwrap(),
            decoded.input_sha256().unwrap()
        );

        let replayed = decoded.evaluate().unwrap();
        let direct = evaluate_preparation(raw, context);
        assert_eq!(direct.report, replayed.report);
        assert_eq!(
            vec!["TITLE_EMPTY", "PRICE_NEGATIVE"],
            replayed
                .report
                .issues()
                .iter()
                .map(|issue| issue.reason.as_str())
                .collect::<Vec<_>>()
        );
    }
}
