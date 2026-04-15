use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

/// Top-level Lemon Squeezy webhook payload.
///
/// See <https://docs.lemonsqueezy.com/guides/developer-guide/webhooks>
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LemonSqueezyWebhook {
    pub meta: WebhookMeta,
    pub data: WebhookData,
}

/// Metadata included with every Lemon Squeezy webhook event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookMeta {
    pub event_name: WebhookEventName,

    #[serde(default)]
    pub test_mode: bool,

    #[serde(default)]
    pub custom_data: Option<CustomData>,
}

/// Custom data passed through checkout.
///
/// See <https://docs.lemonsqueezy.com/help/checkout/passing-custom-data>
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomData {
    #[serde(default, alias = "userId")]
    pub user_id: Option<String>,

    /// Catch-all for any other custom fields.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// The resource data from the webhook event.
///
/// Uses a generic `attributes` map to accommodate all possible resource types
/// (orders, subscriptions, license keys, etc.) without needing to model every
/// single field from every Lemon Squeezy resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookData {
    pub id: serde_json::Value,

    #[serde(rename = "type")]
    pub resource_type: String,

    #[serde(default)]
    pub attributes: HashMap<String, serde_json::Value>,
}

/// All known Lemon Squeezy webhook event types.
///
/// See <https://docs.lemonsqueezy.com/help/webhooks/event-types>
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WebhookEventName {
    // Order events
    OrderCreated,
    OrderRefunded,

    // Customer events
    CustomerUpdated,

    // Subscription events
    SubscriptionCreated,
    SubscriptionUpdated,
    SubscriptionCancelled,
    SubscriptionResumed,
    SubscriptionExpired,
    SubscriptionPaused,
    SubscriptionUnpaused,

    // Subscription payment events
    SubscriptionPaymentSuccess,
    SubscriptionPaymentFailed,
    SubscriptionPaymentRecovered,
    SubscriptionPaymentRefunded,

    // License key events
    LicenseKeyCreated,
    LicenseKeyUpdated,

    /// Catch-all for unknown/future event types.
    Unknown(String),
}

impl fmt::Display for WebhookEventName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OrderCreated => write!(f, "order_created"),
            Self::OrderRefunded => write!(f, "order_refunded"),
            Self::CustomerUpdated => write!(f, "customer_updated"),
            Self::SubscriptionCreated => write!(f, "subscription_created"),
            Self::SubscriptionUpdated => write!(f, "subscription_updated"),
            Self::SubscriptionCancelled => write!(f, "subscription_cancelled"),
            Self::SubscriptionResumed => write!(f, "subscription_resumed"),
            Self::SubscriptionExpired => write!(f, "subscription_expired"),
            Self::SubscriptionPaused => write!(f, "subscription_paused"),
            Self::SubscriptionUnpaused => write!(f, "subscription_unpaused"),
            Self::SubscriptionPaymentSuccess => write!(f, "subscription_payment_success"),
            Self::SubscriptionPaymentFailed => write!(f, "subscription_payment_failed"),
            Self::SubscriptionPaymentRecovered => write!(f, "subscription_payment_recovered"),
            Self::SubscriptionPaymentRefunded => write!(f, "subscription_payment_refunded"),
            Self::LicenseKeyCreated => write!(f, "license_key_created"),
            Self::LicenseKeyUpdated => write!(f, "license_key_updated"),
            Self::Unknown(name) => write!(f, "{name}"),
        }
    }
}

impl Serialize for WebhookEventName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for WebhookEventName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "order_created" => Self::OrderCreated,
            "order_refunded" => Self::OrderRefunded,
            "customer_updated" => Self::CustomerUpdated,
            "subscription_created" => Self::SubscriptionCreated,
            "subscription_updated" => Self::SubscriptionUpdated,
            "subscription_cancelled" => Self::SubscriptionCancelled,
            "subscription_resumed" => Self::SubscriptionResumed,
            "subscription_expired" => Self::SubscriptionExpired,
            "subscription_paused" => Self::SubscriptionPaused,
            "subscription_unpaused" => Self::SubscriptionUnpaused,
            "subscription_payment_success" => Self::SubscriptionPaymentSuccess,
            "subscription_payment_failed" => Self::SubscriptionPaymentFailed,
            "subscription_payment_recovered" => Self::SubscriptionPaymentRecovered,
            "subscription_payment_refunded" => Self::SubscriptionPaymentRefunded,
            "license_key_created" => Self::LicenseKeyCreated,
            "license_key_updated" => Self::LicenseKeyUpdated,
            other => Self::Unknown(other.to_owned()),
        })
    }
}

/// Subscription status values from the Lemon Squeezy API.
///
/// See <https://docs.lemonsqueezy.com/api/subscriptions>
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionStatus {
    Active,
    OnTrial,
    Paused,
    PastDue,
    Unpaid,
    Cancelled,
    Expired,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_deserialize_subscription_created_event() {
        let json = r#"{
            "meta": {
                "event_name": "subscription_created",
                "test_mode": false,
                "custom_data": {
                    "user_id": "550e8400-e29b-41d4-a716-446655440000"
                }
            },
            "data": {
                "id": "123",
                "type": "subscriptions",
                "attributes": {
                    "status": "active",
                    "store_id": 1
                }
            }
        }"#;

        let webhook: LemonSqueezyWebhook = serde_json::from_str(json).unwrap();
        assert_eq!(
            webhook.meta.event_name,
            WebhookEventName::SubscriptionCreated
        );
        assert!(!webhook.meta.test_mode);
        assert_eq!(
            webhook
                .meta
                .custom_data
                .as_ref()
                .unwrap()
                .user_id
                .as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
        assert_eq!(webhook.data.resource_type, "subscriptions");
        assert_eq!(
            webhook.data.attributes.get("status").unwrap().as_str(),
            Some("active")
        );
    }

    #[test]
    fn should_deserialize_unknown_event() {
        let json = r#"{
            "meta": {
                "event_name": "some_future_event"
            },
            "data": {
                "id": "456",
                "type": "some_resource",
                "attributes": {}
            }
        }"#;

        let webhook: LemonSqueezyWebhook = serde_json::from_str(json).unwrap();
        assert_eq!(
            webhook.meta.event_name,
            WebhookEventName::Unknown("some_future_event".to_owned())
        );
    }

    #[test]
    fn should_deserialize_all_known_event_names() {
        let events = [
            ("order_created", WebhookEventName::OrderCreated),
            ("order_refunded", WebhookEventName::OrderRefunded),
            ("customer_updated", WebhookEventName::CustomerUpdated),
            (
                "subscription_created",
                WebhookEventName::SubscriptionCreated,
            ),
            (
                "subscription_updated",
                WebhookEventName::SubscriptionUpdated,
            ),
            (
                "subscription_cancelled",
                WebhookEventName::SubscriptionCancelled,
            ),
            (
                "subscription_resumed",
                WebhookEventName::SubscriptionResumed,
            ),
            (
                "subscription_expired",
                WebhookEventName::SubscriptionExpired,
            ),
            ("subscription_paused", WebhookEventName::SubscriptionPaused),
            (
                "subscription_unpaused",
                WebhookEventName::SubscriptionUnpaused,
            ),
            (
                "subscription_payment_success",
                WebhookEventName::SubscriptionPaymentSuccess,
            ),
            (
                "subscription_payment_failed",
                WebhookEventName::SubscriptionPaymentFailed,
            ),
            (
                "subscription_payment_recovered",
                WebhookEventName::SubscriptionPaymentRecovered,
            ),
            (
                "subscription_payment_refunded",
                WebhookEventName::SubscriptionPaymentRefunded,
            ),
            ("license_key_created", WebhookEventName::LicenseKeyCreated),
            ("license_key_updated", WebhookEventName::LicenseKeyUpdated),
        ];

        for (name, expected) in events {
            let json = format!(
                r#"{{"meta":{{"event_name":"{name}"}},"data":{{"id":"1","type":"t","attributes":{{}}}}}}"#,
            );
            let webhook: LemonSqueezyWebhook = serde_json::from_str(&json).unwrap();
            assert_eq!(
                webhook.meta.event_name, expected,
                "Failed for event: {name}"
            );
        }
    }

    #[test]
    fn should_roundtrip_serialize_event_names() {
        let events = [
            WebhookEventName::OrderCreated,
            WebhookEventName::OrderRefunded,
            WebhookEventName::CustomerUpdated,
            WebhookEventName::SubscriptionCreated,
            WebhookEventName::SubscriptionUpdated,
            WebhookEventName::SubscriptionCancelled,
            WebhookEventName::SubscriptionResumed,
            WebhookEventName::SubscriptionExpired,
            WebhookEventName::SubscriptionPaused,
            WebhookEventName::SubscriptionUnpaused,
            WebhookEventName::SubscriptionPaymentSuccess,
            WebhookEventName::SubscriptionPaymentFailed,
            WebhookEventName::SubscriptionPaymentRecovered,
            WebhookEventName::SubscriptionPaymentRefunded,
            WebhookEventName::LicenseKeyCreated,
            WebhookEventName::LicenseKeyUpdated,
            WebhookEventName::Unknown("some_future_event".to_owned()),
        ];

        for event in events {
            let serialized = serde_json::to_string(&event).unwrap();
            let deserialized: WebhookEventName = serde_json::from_str(&serialized).unwrap();
            assert_eq!(event, deserialized);
        }
    }

    #[test]
    fn should_deserialize_all_subscription_statuses() {
        let statuses = [
            ("active", SubscriptionStatus::Active),
            ("on_trial", SubscriptionStatus::OnTrial),
            ("paused", SubscriptionStatus::Paused),
            ("past_due", SubscriptionStatus::PastDue),
            ("unpaid", SubscriptionStatus::Unpaid),
            ("cancelled", SubscriptionStatus::Cancelled),
            ("expired", SubscriptionStatus::Expired),
        ];

        for (name, expected) in statuses {
            let value = serde_json::Value::String(name.to_owned());
            let status: SubscriptionStatus = serde_json::from_value(value).unwrap();
            assert_eq!(status, expected, "Failed for status: {name}");
        }
    }

    #[test]
    fn should_deserialize_webhook_without_custom_data() {
        let json = r#"{
            "meta": {
                "event_name": "order_created"
            },
            "data": {
                "id": "789",
                "type": "orders",
                "attributes": {}
            }
        }"#;

        let webhook: LemonSqueezyWebhook = serde_json::from_str(json).unwrap();
        assert!(webhook.meta.custom_data.is_none());
    }

    #[test]
    fn should_deserialize_custom_data_with_extra_fields() {
        let json = r#"{
            "meta": {
                "event_name": "subscription_created",
                "custom_data": {
                    "user_id": "550e8400-e29b-41d4-a716-446655440000",
                    "campaign": "spring2025"
                }
            },
            "data": {
                "id": "1",
                "type": "subscriptions",
                "attributes": {}
            }
        }"#;

        let webhook: LemonSqueezyWebhook = serde_json::from_str(json).unwrap();
        let custom = webhook.meta.custom_data.unwrap();
        assert_eq!(
            custom.user_id.as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
        assert_eq!(
            custom.extra.get("campaign").unwrap().as_str(),
            Some("spring2025")
        );
    }

    #[test]
    fn should_deserialize_custom_data_with_camel_case_user_id() {
        let json = r#"{
            "meta": {
                "event_name": "subscription_created",
                "custom_data": {
                    "userId": "550e8400-e29b-41d4-a716-446655440000"
                }
            },
            "data": {
                "id": "1",
                "type": "subscriptions",
                "attributes": {}
            }
        }"#;

        let webhook: LemonSqueezyWebhook = serde_json::from_str(json).unwrap();
        let custom = webhook.meta.custom_data.unwrap();
        assert_eq!(
            custom.user_id.as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
    }

    #[test]
    fn should_deserialize_webhook_with_numeric_data_id() {
        let json = r#"{
            "meta": {
                "event_name": "order_created"
            },
            "data": {
                "id": 42,
                "type": "orders",
                "attributes": {
                    "total": 2999
                }
            }
        }"#;

        let webhook: LemonSqueezyWebhook = serde_json::from_str(json).unwrap();
        assert_eq!(webhook.data.id.as_u64(), Some(42));
    }

    #[test]
    fn should_display_all_event_names() {
        assert_eq!(WebhookEventName::OrderCreated.to_string(), "order_created");
        assert_eq!(
            WebhookEventName::SubscriptionPaymentRecovered.to_string(),
            "subscription_payment_recovered"
        );
        assert_eq!(
            WebhookEventName::Unknown("custom_event".to_owned()).to_string(),
            "custom_event"
        );
    }
}
