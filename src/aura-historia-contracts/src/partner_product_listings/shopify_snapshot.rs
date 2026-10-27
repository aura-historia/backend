//! Publication-filtered Shopify bulk intake. The selection timestamp is stable on retries.
use serde::Deserialize;
use serde_json::Value;
use time::OffsetDateTime;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShopifySnapshotData {
    pub domain: String,
    #[serde(with = "time::serde::rfc3339")]
    pub snapshot_started_at: OffsetDateTime,
    pub selection_complete: bool,
    pub items: Vec<ShopifySnapshotItemData>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShopifySnapshotItemData {
    pub operation: ShopifySnapshotOperation,
    pub payload: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ShopifySnapshotOperation {
    Upsert,
    Withdraw,
}
