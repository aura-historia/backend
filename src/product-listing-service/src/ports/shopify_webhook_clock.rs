use time::OffsetDateTime;

/// The time a Shopify delivery is received at, so tests can control the trigger-time window.
pub trait ShopifyWebhookClock: Send + Sync {
    fn now_utc(&self) -> OffsetDateTime;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemShopifyWebhookClock;

impl ShopifyWebhookClock for SystemShopifyWebhookClock {
    fn now_utc(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}
