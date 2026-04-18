//! AWS Lambda handler that processes Stripe events delivered through the
//! EventBridge integration documented at
//! <https://docs.stripe.com/event-destinations/eventbridge>.
//!
//! Stripe delivers each event onto a Partner Event Bus.  An EventBridge rule
//! forwards events whose Stripe `type` (extracted from `detail.type`) matches
//! one of the supported subscription lifecycle event-types to this Lambda.
//! The detail of each EventBridge envelope contains the raw Stripe
//! event-object.
//!
//! The handler is intentionally robust:
//!
//! * unknown / unhandled `detail.type` values are logged and skipped without
//!   failing the invocation,
//! * missing or malformed Stripe-fields (for example `customer`, `metadata`,
//!   product-ids, ...) result in a structured error log; the invocation
//!   succeeds so the message is *not* re-delivered indefinitely,
//! * personal data (the `metadata.userId`, the Stripe customer-id) is **not**
//!   logged.  Errors only reference the affected resource via opaque
//!   identifiers (`subscriptionId`, the event-id) so logs stay PII-free.

use aws_lambda_events::eventbridge::EventBridgeEvent;
use common::{stripe_customer_id::StripeCustomerId, user_id::UserId};
use lambda_runtime::LambdaEvent;
use serde::Deserialize;
use serde_json::Value;
use tracing::{debug, error, info, warn};
use user::{
    core::tier::UserTier,
    service::{
        command::UpdateUserCommand,
        user_service::{UserService, UserServiceError},
    },
};

/// Stripe event-type values handled explicitly by this Lambda. These are read
/// from the Stripe event's `type` field (which arrives at `detail.type` inside
/// the EventBridge envelope).
pub const STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED: &str = "customer.subscription.created";
pub const STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED: &str = "customer.subscription.updated";
pub const STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED: &str = "customer.subscription.deleted";

/// Mapping from Stripe product-ids to internal [`UserTier`].
///
/// The `stripe-lambda` runtime constructs this from environment variables; the
/// type itself is kept generic so unit-tests do not need any environment
/// setup.
#[derive(Debug, Clone)]
pub struct StripeProductTierMap {
    pub pro_product_id: String,
    pub ultimate_product_id: String,
}

impl StripeProductTierMap {
    /// Resolve a Stripe product-id to a [`UserTier`], returning `None` for
    /// product-ids that do not map to a known plan.
    pub fn tier_for(&self, product_id: &str) -> Option<UserTier> {
        if product_id == self.pro_product_id {
            Some(UserTier::Pro)
        } else if product_id == self.ultimate_product_id {
            Some(UserTier::Ultimate)
        } else {
            None
        }
    }
}

/// Subset of a Stripe `Subscription` object as documented at
/// <https://docs.stripe.com/api/subscriptions/object>. Only the fields needed
/// to react to subscription lifecycle events are deserialized.
#[derive(Debug, Clone, Deserialize)]
pub struct StripeSubscription {
    /// The Stripe subscription id (`sub_…`). Used for logging only.
    #[serde(default)]
    pub id: Option<String>,
    /// The Stripe customer id (`cus_…`).
    #[serde(default)]
    pub customer: Option<String>,
    /// Free-form metadata Stripe keeps on the subscription. We expect a
    /// `userId` to be supplied by the frontend on first checkout.
    #[serde(default)]
    pub metadata: std::collections::BTreeMap<String, String>,
    /// Subscription items list (`items.data`).
    #[serde(default)]
    pub items: Option<StripeSubscriptionItems>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StripeSubscriptionItems {
    #[serde(default)]
    pub data: Vec<StripeSubscriptionItem>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StripeSubscriptionItem {
    #[serde(default)]
    pub price: Option<StripePrice>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StripePrice {
    /// The Stripe product-id (`prod_…`).
    #[serde(default)]
    pub product: Option<String>,
}

impl StripeSubscription {
    /// Returns the Stripe product-id of the first subscription-item, if any.
    pub fn first_product_id(&self) -> Option<&str> {
        self.items
            .as_ref()
            .and_then(|i| i.data.first())
            .and_then(|d| d.price.as_ref())
            .and_then(|p| p.product.as_deref())
    }

    /// Parses `metadata.userId` as a `UserId`.
    pub fn user_id_from_metadata(&self) -> Option<UserId> {
        self.metadata
            .get("userId")
            .and_then(|s| UserId::try_from(s.as_str()).ok())
    }

    /// Returns `customer` as a `StripeCustomerId` when present and non-empty.
    pub fn stripe_customer_id(&self) -> Option<StripeCustomerId> {
        self.customer
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(StripeCustomerId::from)
    }
}

/// Extracts the Stripe event-type from the EventBridge `detail.type` field.
fn stripe_event_type(detail: &Value) -> Option<&str> {
    detail.get("type").and_then(|v| v.as_str())
}

#[tracing::instrument(
    skip(event, service, tier_map),
    fields(
        requestId = %event.context.request_id,
        eventId = ?event.payload.id,
        source = %event.payload.source,
    )
)]
pub async fn handler(
    event: LambdaEvent<EventBridgeEvent<Value>>,
    service: &impl UserService,
    tier_map: &StripeProductTierMap,
) -> Result<(), lambda_runtime::Error> {
    let payload = event.payload;

    let Some(stripe_type) = stripe_event_type(&payload.detail) else {
        warn!("Stripe event is missing 'type' field, ignoring.");
        return Ok(());
    };

    match stripe_type {
        STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED => {
            handle_subscription_created(&payload.detail, service, tier_map).await
        }
        STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED => {
            handle_subscription_updated(&payload.detail, service, tier_map).await
        }
        STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED => {
            handle_subscription_deleted(&payload.detail, service).await
        }
        other => {
            warn!(
                stripeType = %other,
                "Received unsupported Stripe event-type, ignoring."
            );
            Ok(())
        }
    }
}

/// Best-effort deserialization of the EventBridge `detail` into a
/// [`StripeSubscription`]. Stripe wraps the subscription in a
/// `data.object` envelope; we additionally accept the bare object for
/// resilience to any future EventBridge format changes.
fn parse_subscription(detail: &Value) -> Option<StripeSubscription> {
    let candidate = detail
        .get("data")
        .and_then(|d| d.get("object"))
        .unwrap_or(detail);

    match serde_json::from_value::<StripeSubscription>(candidate.clone()) {
        Ok(sub) => Some(sub),
        Err(err) => {
            error!(
                error = %err,
                "Failed deserializing Stripe subscription from EventBridge detail."
            );
            None
        }
    }
}

async fn handle_subscription_created(
    detail: &Value,
    service: &impl UserService,
    tier_map: &StripeProductTierMap,
) -> Result<(), lambda_runtime::Error> {
    let Some(subscription) = parse_subscription(detail) else {
        return Ok(());
    };

    let subscription_id = subscription.id.as_deref().unwrap_or("<unknown>");

    let Some(user_id) = subscription.user_id_from_metadata() else {
        error!(
            subscriptionId = %subscription_id,
            "Stripe subscription-created is missing or has invalid 'metadata.userId' — cannot \
             associate with a user. Skipping."
        );
        return Ok(());
    };

    let Some(stripe_customer_id) = subscription.stripe_customer_id() else {
        error!(
            subscriptionId = %subscription_id,
            "Stripe subscription-created is missing 'customer'. Skipping."
        );
        return Ok(());
    };

    let Some(product_id) = subscription.first_product_id() else {
        error!(
            subscriptionId = %subscription_id,
            "Stripe subscription-created has no items[0].price.product. Skipping."
        );
        return Ok(());
    };

    let Some(tier) = tier_map.tier_for(product_id) else {
        warn!(
            subscriptionId = %subscription_id,
            "Stripe subscription-created references unknown Stripe product-id — no tier \
             change applied."
        );
        return Ok(());
    };

    let cmd = UpdateUserCommand {
        tier: Some(tier),
        stripe_customer_id: Some(stripe_customer_id),
        ..Default::default()
    };
    match service.update_user(&user_id, cmd).await {
        Ok(_) => {
            info!(
                subscriptionId = %subscription_id,
                tier = ?tier,
                "Stripe subscription-created processed."
            );
            Ok(())
        }
        Err(UserServiceError::UserNotFound(_)) => {
            error!(
                subscriptionId = %subscription_id,
                "Stripe subscription-created references unknown user (metadata.userId). Skipping."
            );
            Ok(())
        }
        Err(err) => {
            error!(error = %err, subscriptionId = %subscription_id, "Failed to apply Stripe subscription-created.");
            Err(Box::new(err))
        }
    }
}

async fn handle_subscription_updated(
    detail: &Value,
    service: &impl UserService,
    tier_map: &StripeProductTierMap,
) -> Result<(), lambda_runtime::Error> {
    let Some(subscription) = parse_subscription(detail) else {
        return Ok(());
    };

    let subscription_id = subscription.id.as_deref().unwrap_or("<unknown>");

    let Some(stripe_customer_id) = subscription.stripe_customer_id() else {
        error!(
            subscriptionId = %subscription_id,
            "Stripe subscription-updated is missing 'customer'. Skipping."
        );
        return Ok(());
    };

    let Some(product_id) = subscription.first_product_id() else {
        error!(
            subscriptionId = %subscription_id,
            "Stripe subscription-updated has no items[0].price.product. Skipping."
        );
        return Ok(());
    };

    let Some(tier) = tier_map.tier_for(product_id) else {
        warn!(
            subscriptionId = %subscription_id,
            "Stripe subscription-updated references unknown Stripe product-id — no tier \
             change applied."
        );
        return Ok(());
    };

    apply_tier_change_by_customer_id(
        service,
        &stripe_customer_id,
        tier,
        subscription_id,
        "subscription-updated",
    )
    .await
}

async fn handle_subscription_deleted(
    detail: &Value,
    service: &impl UserService,
) -> Result<(), lambda_runtime::Error> {
    let Some(subscription) = parse_subscription(detail) else {
        return Ok(());
    };

    let subscription_id = subscription.id.as_deref().unwrap_or("<unknown>");

    let Some(stripe_customer_id) = subscription.stripe_customer_id() else {
        error!(
            subscriptionId = %subscription_id,
            "Stripe subscription-deleted is missing 'customer'. Skipping."
        );
        return Ok(());
    };

    apply_tier_change_by_customer_id(
        service,
        &stripe_customer_id,
        UserTier::Free,
        subscription_id,
        "subscription-deleted",
    )
    .await
}

/// Looks up the user by Stripe customer-id and updates the tier. Used by both
/// `subscription-updated` and `subscription-deleted`. The Stripe customer-id
/// itself is intentionally **not** modified — Stripe customers persist across
/// the subscription lifecycle.
async fn apply_tier_change_by_customer_id(
    service: &impl UserService,
    stripe_customer_id: &StripeCustomerId,
    tier: UserTier,
    subscription_id: &str,
    event_label: &str,
) -> Result<(), lambda_runtime::Error> {
    let user = match service
        .find_user_by_stripe_customer_id(stripe_customer_id)
        .await
    {
        Ok(user) => user,
        Err(UserServiceError::UserNotFoundByStripeCustomerId) => {
            error!(
                subscriptionId = %subscription_id,
                "Stripe {event_label} references unknown Stripe customer-id. Skipping."
            );
            return Ok(());
        }
        Err(err) => {
            error!(error = %err, subscriptionId = %subscription_id, "Failed to look up user by Stripe customer-id for {event_label}.");
            return Err(Box::new(err));
        }
    };

    let cmd = UpdateUserCommand {
        tier: Some(tier),
        ..Default::default()
    };
    match service.update_user(&user.user_id, cmd).await {
        Ok(_) => {
            debug!(
                subscriptionId = %subscription_id,
                tier = ?tier,
                "Stripe {event_label} processed."
            );
            Ok(())
        }
        Err(err) => {
            error!(error = %err, subscriptionId = %subscription_id, "Failed to apply Stripe {event_label}.");
            Err(Box::new(err))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lambda_events::eventbridge::EventBridgeEvent;
    use lambda_runtime::Context;
    use serde_json::json;
    use user::core::user::User;
    use user::service::user_service::MockUserService;

    fn tier_map() -> StripeProductTierMap {
        StripeProductTierMap {
            pro_product_id: "prod_pro".into(),
            ultimate_product_id: "prod_ultimate".into(),
        }
    }

    /// Builds an EventBridge envelope around the supplied `detail` payload,
    /// injecting the Stripe event-type into `detail.type` (mirroring how Stripe
    /// delivers events through EventBridge).
    fn lambda_event(stripe_type: &str, detail: Value) -> LambdaEvent<EventBridgeEvent<Value>> {
        let mut detail = detail;
        if let Some(obj) = detail.as_object_mut() {
            obj.insert("type".to_string(), json!(stripe_type));
        }
        let mut event = EventBridgeEvent::<Value>::default();
        event.detail_type = stripe_type.to_string();
        event.source = "aws.partner/stripe.com/test".to_string();
        event.detail = detail;
        LambdaEvent::new(event, Context::default())
    }

    fn dummy_user() -> User {
        use fake::{Fake, Faker};
        Faker.fake::<User>()
    }

    #[test]
    fn should_map_stripe_product_id_to_pro_when_matches_pro_id() {
        let map = tier_map();
        assert_eq!(map.tier_for("prod_pro"), Some(UserTier::Pro));
    }

    #[test]
    fn should_map_stripe_product_id_to_ultimate_when_matches_ultimate_id() {
        let map = tier_map();
        assert_eq!(map.tier_for("prod_ultimate"), Some(UserTier::Ultimate));
    }

    #[test]
    fn should_return_none_when_product_id_unknown_for_tier_lookup() {
        let map = tier_map();
        assert_eq!(map.tier_for("prod_unknown"), None);
        assert_eq!(map.tier_for(""), None);
    }

    #[test]
    fn should_extract_user_id_from_metadata_when_valid_uuid() {
        let user_id = UserId::new();
        let detail = json!({
            "data": { "object": {
                "id": "sub_1",
                "customer": "cus_1",
                "metadata": { "userId": user_id.to_string() }
            }}
        });
        let sub = parse_subscription(&detail).unwrap();
        assert_eq!(sub.user_id_from_metadata(), Some(user_id));
    }

    #[test]
    fn should_return_none_user_id_when_metadata_missing() {
        let sub: StripeSubscription = serde_json::from_value(json!({"customer": "cus_1"})).unwrap();
        assert_eq!(sub.user_id_from_metadata(), None);
    }

    #[test]
    fn should_return_none_user_id_when_metadata_invalid_uuid() {
        let sub: StripeSubscription =
            serde_json::from_value(json!({"metadata": {"userId": "not-a-uuid"}})).unwrap();
        assert_eq!(sub.user_id_from_metadata(), None);
    }

    #[test]
    fn should_return_none_customer_when_customer_missing_or_empty() {
        let sub: StripeSubscription = serde_json::from_value(json!({})).unwrap();
        assert!(sub.stripe_customer_id().is_none());

        let sub: StripeSubscription = serde_json::from_value(json!({"customer": ""})).unwrap();
        assert!(sub.stripe_customer_id().is_none());
    }

    #[test]
    fn should_extract_first_product_id_from_items() {
        let sub: StripeSubscription = serde_json::from_value(json!({
            "items": { "data": [
                {"price": {"product": "prod_pro"}},
                {"price": {"product": "prod_other"}}
            ]}
        }))
        .unwrap();
        assert_eq!(sub.first_product_id(), Some("prod_pro"));
    }

    #[test]
    fn should_return_none_first_product_id_when_items_empty() {
        let sub: StripeSubscription =
            serde_json::from_value(json!({"items": {"data": []}})).unwrap();
        assert_eq!(sub.first_product_id(), None);
    }

    #[test]
    fn should_parse_bare_subscription_object_when_no_envelope() {
        let detail = json!({"id": "sub_1", "customer": "cus_1"});
        let sub = parse_subscription(&detail).unwrap();
        assert_eq!(sub.id.as_deref(), Some("sub_1"));
    }

    #[tokio::test]
    async fn should_skip_unsupported_detail_type_without_calling_service() {
        let service = MockUserService::default(); // no expectations
        let map = tier_map();
        let event = lambda_event("invoice.paid", json!({}));

        let actual = handler(event, &service, &map).await;
        assert!(actual.is_ok());
    }

    #[tokio::test]
    async fn should_skip_when_stripe_event_type_missing_from_detail() {
        let service = MockUserService::default();
        let map = tier_map();
        let mut event = EventBridgeEvent::<Value>::default();
        event.detail_type = STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED.to_string();
        event.detail = json!({"data": {"object": {}}}); // no `type` field
        let actual = handler(LambdaEvent::new(event, Context::default()), &service, &map).await;
        assert!(actual.is_ok());
    }

    #[tokio::test]
    async fn should_apply_subscription_created_when_payload_valid() {
        let user_id = UserId::new();
        let map = tier_map();
        let event = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED,
            json!({"data": {"object": {
                "id": "sub_1",
                "customer": "cus_1",
                "metadata": {"userId": user_id.to_string()},
                "items": {"data": [{"price": {"product": "prod_pro"}}]}
            }}}),
        );
        let mut service = MockUserService::default();
        let user = dummy_user();
        service
            .expect_update_user()
            .withf(move |uid, cmd| {
                *uid == user_id
                    && cmd.tier == Some(UserTier::Pro)
                    && cmd.stripe_customer_id.as_ref().map(|s| s.as_ref()) == Some("cus_1")
            })
            .return_once(move |_, _| Box::pin(async move { Ok(user) }));

        let actual = handler(event, &service, &map).await;
        assert!(actual.is_ok());
    }

    #[tokio::test]
    async fn should_skip_subscription_created_when_user_id_missing_from_metadata() {
        let map = tier_map();
        let event = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED,
            json!({"data": {"object": {
                "id": "sub_1",
                "customer": "cus_1",
                "items": {"data": [{"price": {"product": "prod_pro"}}]}
            }}}),
        );
        let service = MockUserService::default();
        let actual = handler(event, &service, &map).await;
        assert!(actual.is_ok());
    }

    #[tokio::test]
    async fn should_skip_subscription_created_when_user_id_invalid_uuid() {
        let map = tier_map();
        let event = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED,
            json!({"data": {"object": {
                "id": "sub_1",
                "customer": "cus_1",
                "metadata": {"userId": "not-a-uuid"},
                "items": {"data": [{"price": {"product": "prod_pro"}}]}
            }}}),
        );
        let service = MockUserService::default();
        let actual = handler(event, &service, &map).await;
        assert!(actual.is_ok());
    }

    #[tokio::test]
    async fn should_skip_subscription_created_when_customer_missing() {
        let user_id = UserId::new();
        let map = tier_map();
        let event = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED,
            json!({"data": {"object": {
                "id": "sub_1",
                "metadata": {"userId": user_id.to_string()},
                "items": {"data": [{"price": {"product": "prod_pro"}}]}
            }}}),
        );
        let service = MockUserService::default();
        let actual = handler(event, &service, &map).await;
        assert!(actual.is_ok());
    }

    #[tokio::test]
    async fn should_skip_subscription_created_when_product_id_unknown() {
        let user_id = UserId::new();
        let map = tier_map();
        let event = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED,
            json!({"data": {"object": {
                "id": "sub_1",
                "customer": "cus_1",
                "metadata": {"userId": user_id.to_string()},
                "items": {"data": [{"price": {"product": "prod_unknown"}}]}
            }}}),
        );
        let service = MockUserService::default();
        let actual = handler(event, &service, &map).await;
        assert!(actual.is_ok());
    }

    #[tokio::test]
    async fn should_skip_subscription_created_when_user_not_found() {
        let user_id = UserId::new();
        let map = tier_map();
        let event = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED,
            json!({"data": {"object": {
                "id": "sub_1",
                "customer": "cus_1",
                "metadata": {"userId": user_id.to_string()},
                "items": {"data": [{"price": {"product": "prod_ultimate"}}]}
            }}}),
        );
        let mut service = MockUserService::default();
        service.expect_update_user().return_once(move |uid, _| {
            let uid = *uid;
            Box::pin(async move { Err(UserServiceError::UserNotFound(uid)) })
        });

        let actual = handler(event, &service, &map).await;
        assert!(actual.is_ok());
    }

    #[tokio::test]
    async fn should_apply_subscription_updated_when_payload_valid() {
        let map = tier_map();
        let event = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED,
            json!({"data": {"object": {
                "id": "sub_1",
                "customer": "cus_1",
                "items": {"data": [{"price": {"product": "prod_ultimate"}}]}
            }}}),
        );
        let mut service = MockUserService::default();
        let user = dummy_user();
        let user_id = user.user_id;
        let user_for_lookup = user.clone();
        service
            .expect_find_user_by_stripe_customer_id()
            .withf(|scid| scid.as_ref() == "cus_1")
            .return_once(move |_| Box::pin(async move { Ok(user_for_lookup) }));
        service
            .expect_update_user()
            .withf(move |uid, cmd| {
                *uid == user_id
                    && cmd.tier == Some(UserTier::Ultimate)
                    && cmd.stripe_customer_id.is_none()
            })
            .return_once(move |_, _| Box::pin(async move { Ok(user) }));
        let actual = handler(event, &service, &map).await;
        assert!(actual.is_ok());
    }

    #[tokio::test]
    async fn should_skip_subscription_updated_when_customer_missing() {
        let map = tier_map();
        let event = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED,
            json!({"data": {"object": {
                "id": "sub_1",
                "items": {"data": [{"price": {"product": "prod_ultimate"}}]}
            }}}),
        );
        let service = MockUserService::default();
        let actual = handler(event, &service, &map).await;
        assert!(actual.is_ok());
    }

    #[tokio::test]
    async fn should_skip_subscription_updated_when_user_not_found_by_customer_id() {
        let map = tier_map();
        let event = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED,
            json!({"data": {"object": {
                "id": "sub_1",
                "customer": "cus_1",
                "items": {"data": [{"price": {"product": "prod_ultimate"}}]}
            }}}),
        );
        let mut service = MockUserService::default();
        service
            .expect_find_user_by_stripe_customer_id()
            .return_once(|_| {
                Box::pin(async { Err(UserServiceError::UserNotFoundByStripeCustomerId) })
            });
        let actual = handler(event, &service, &map).await;
        assert!(actual.is_ok());
    }

    #[tokio::test]
    async fn should_apply_subscription_deleted_when_payload_valid() {
        let map = tier_map();
        let event = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED,
            json!({"data": {"object": {
                "id": "sub_1",
                "customer": "cus_1"
            }}}),
        );
        let mut service = MockUserService::default();
        let user = dummy_user();
        let user_id = user.user_id;
        let user_for_lookup = user.clone();
        service
            .expect_find_user_by_stripe_customer_id()
            .withf(|scid| scid.as_ref() == "cus_1")
            .return_once(move |_| Box::pin(async move { Ok(user_for_lookup) }));
        service
            .expect_update_user()
            .withf(move |uid, cmd| {
                *uid == user_id
                    && cmd.tier == Some(UserTier::Free)
                    && cmd.stripe_customer_id.is_none()
            })
            .return_once(move |_, _| Box::pin(async move { Ok(user) }));
        let actual = handler(event, &service, &map).await;
        assert!(actual.is_ok());
    }

    #[tokio::test]
    async fn should_skip_subscription_deleted_when_customer_missing() {
        let map = tier_map();
        let event = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED,
            json!({"data": {"object": {"id": "sub_1"}}}),
        );
        let service = MockUserService::default();
        let actual = handler(event, &service, &map).await;
        assert!(actual.is_ok());
    }

    #[tokio::test]
    async fn should_skip_subscription_deleted_when_user_not_found_by_customer_id() {
        let map = tier_map();
        let event = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED,
            json!({"data": {"object": {"id": "sub_1", "customer": "cus_1"}}}),
        );
        let mut service = MockUserService::default();
        service
            .expect_find_user_by_stripe_customer_id()
            .return_once(|_| {
                Box::pin(async { Err(UserServiceError::UserNotFoundByStripeCustomerId) })
            });
        let actual = handler(event, &service, &map).await;
        assert!(actual.is_ok());
    }

    #[tokio::test]
    async fn should_propagate_unexpected_service_error_for_subscription_deleted() {
        use aws_sdk_dynamodb::config::http::HttpResponse;
        use aws_sdk_dynamodb::error::SdkError;
        use aws_sdk_dynamodb::operation::query::QueryError;
        let map = tier_map();
        let event = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED,
            json!({"data": {"object": {"id": "sub_1", "customer": "cus_1"}}}),
        );
        let mut service = MockUserService::default();
        service
            .expect_find_user_by_stripe_customer_id()
            .return_once(|_| {
                Box::pin(async {
                    Err(UserServiceError::SdkQueryError(SdkError::service_error(
                        QueryError::unhandled("boom"),
                        HttpResponse::new(500u16.try_into().unwrap(), "{}".into()),
                    )))
                })
            });
        let actual = handler(event, &service, &map).await;
        assert!(actual.is_err());
    }

    #[tokio::test]
    async fn should_succeed_when_detail_is_not_a_json_object() {
        let map = tier_map();
        let mut event = EventBridgeEvent::<Value>::default();
        event.detail_type = STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED.to_string();
        event.detail = json!("not-an-object");
        let service = MockUserService::default();
        let actual = handler(LambdaEvent::new(event, Context::default()), &service, &map).await;
        assert!(actual.is_ok());
    }
}
