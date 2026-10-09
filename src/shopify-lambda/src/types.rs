use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ShopifyEventDetail {
    pub(crate) payload: Value,
    pub(crate) metadata: ShopifyEventMetadata,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ShopifyEventMetadata {
    #[serde(rename = "X-Shopify-Topic")]
    pub(crate) topic: String,
    #[serde(rename = "X-Shopify-Shop-Domain")]
    pub(crate) shop_domain: String,
    #[serde(rename = "X-Shopify-Event-Id", default)]
    pub(crate) event_id: Option<String>,
    #[serde(rename = "X-Shopify-Webhook-Id", default)]
    pub(crate) webhook_id: Option<String>,
    #[serde(rename = "X-Shopify-Triggered-At", default)]
    pub(crate) triggered_at: Option<String>,
}
