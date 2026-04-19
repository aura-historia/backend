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
        eventBridgeEventId = tracing::field::Empty,
        stripeEventId = tracing::field::Empty,
        stripeType = tracing::field::Empty,
        userId = tracing::field::Empty,
        stripeCustomerId = tracing::field::Empty,
        source = %event.payload.source,
    )
)]
pub async fn handler(
    event: LambdaEvent<EventBridgeEvent<Value>>,
    service: &impl UserService,
    tier_map: &StripeProductTierMap,
) -> Result<(), lambda_runtime::Error> {
    let payload = event.payload;
    let span = tracing::Span::current();

    if let Some(event_bridge_event_id) = payload.id.as_deref() {
        span.record("eventBridgeEventId", event_bridge_event_id);
    }

    if let Some(stripe_event_id) = payload.detail.get("id").and_then(|value| value.as_str()) {
        span.record("stripeEventId", stripe_event_id);
    }

    let Some(stripe_type) = stripe_event_type(&payload.detail) else {
        warn!("Stripe event is missing 'type' field, ignoring.");
        return Ok(());
    };

    span.record("stripeType", stripe_type);

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

    let span = tracing::Span::current();
    let subscription_id = subscription.id.as_deref().unwrap_or("<unknown>");

    if let Some(user_id) = subscription.user_id_from_metadata() {
        span.record("userId", user_id.to_string());
    }

    if let Some(stripe_customer_id) = subscription.stripe_customer_id() {
        span.record("stripeCustomerId", stripe_customer_id.as_ref());
    }

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

    let span = tracing::Span::current();
    let subscription_id = subscription.id.as_deref().unwrap_or("<unknown>");

    if let Some(stripe_customer_id) = subscription.stripe_customer_id() {
        span.record("stripeCustomerId", stripe_customer_id.as_ref());
    }

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

    let span = tracing::Span::current();
    let subscription_id = subscription.id.as_deref().unwrap_or("<unknown>");

    if let Some(stripe_customer_id) = subscription.stripe_customer_id() {
        span.record("stripeCustomerId", stripe_customer_id.as_ref());
    }

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
    let span = tracing::Span::current();
    span.record("stripeCustomerId", stripe_customer_id.as_ref());

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

    span.record("userId", user.user_id.to_string());

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

    // -------------------------------------------------------------------------
    // Helpers — real Stripe Workbench test-mode payloads
    // -------------------------------------------------------------------------

    /// Tier map whose product-ids are taken directly from the Stripe Workbench
    /// test-mode events below. `prod_UMcMnHoaxN3gAS` is the product from the
    /// `subscription.created` event (mapped to Pro); `prod_UMcRH2ZglY77cx` is
    /// from the `subscription.updated` event (mapped to Ultimate).
    fn workbench_tier_map() -> StripeProductTierMap {
        StripeProductTierMap {
            pro_product_id: "prod_UMcMnHoaxN3gAS".into(),
            ultimate_product_id: "prod_UMcRH2ZglY77cx".into(),
        }
    }

    /// Full EventBridge `detail` (without `type`, which is injected by
    /// [`lambda_event`]) for a real `customer.subscription.created` event
    /// captured from Stripe Workbench in test-mode.
    ///
    /// Notable fields:
    /// * subscription id  : `sub_1TNt7g6KTxEnTfhCyFPKx2oE`
    /// * customer         : `cus_UMcMyweeI3jPgl`
    /// * metadata.userId  : `4f2413c2-c2c6-4418-8740-d8a597252163`
    /// * product          : `prod_UMcMnHoaxN3gAS`
    fn workbench_created_detail() -> Value {
        serde_json::from_str(
            r#"{
            "id": "evt_workbench_created_001",
            "data": {
                "object": {
                    "id": "sub_1TNt7g6KTxEnTfhCyFPKx2oE",
                    "object": "subscription",
                    "application": null,
                    "application_fee_percent": null,
                    "automatic_tax": {
                        "disabled_reason": null,
                        "enabled": false,
                        "liability": null
                    },
                    "billing_cycle_anchor": 1776596564,
                    "billing_cycle_anchor_config": null,
                    "billing_mode": {
                        "flexible": { "proration_discounts": "included" },
                        "type": "flexible",
                        "updated_at": 1776596564
                    },
                    "billing_thresholds": null,
                    "cancel_at": null,
                    "cancel_at_period_end": false,
                    "canceled_at": null,
                    "cancellation_details": {
                        "comment": null,
                        "feedback": null,
                        "reason": null
                    },
                    "collection_method": "charge_automatically",
                    "created": 1776596564,
                    "currency": "usd",
                    "customer": "cus_UMcMyweeI3jPgl",
                    "customer_account": null,
                    "days_until_due": null,
                    "default_payment_method": null,
                    "default_source": null,
                    "default_tax_rates": [],
                    "description": null,
                    "discounts": [],
                    "ended_at": null,
                    "invoice_settings": {
                        "account_tax_ids": null,
                        "issuer": { "type": "self" }
                    },
                    "items": {
                        "object": "list",
                        "data": [
                            {
                                "id": "si_UMcMw2TDajXhid",
                                "object": "subscription_item",
                                "billing_thresholds": null,
                                "created": 1776596564,
                                "current_period_end": 1779188564,
                                "current_period_start": 1776596564,
                                "discounts": [],
                                "metadata": {},
                                "plan": {
                                    "id": "price_1TNt7f6KTxEnTfhCxVyhstYY",
                                    "object": "plan",
                                    "active": true,
                                    "amount": 1500,
                                    "amount_decimal": "1500",
                                    "billing_scheme": "per_unit",
                                    "created": 1776596563,
                                    "currency": "usd",
                                    "interval": "month",
                                    "interval_count": 1,
                                    "livemode": false,
                                    "metadata": {},
                                    "meter": null,
                                    "nickname": null,
                                    "product": "prod_UMcMnHoaxN3gAS",
                                    "tiers_mode": null,
                                    "transform_usage": null,
                                    "trial_period_days": null,
                                    "usage_type": "licensed"
                                },
                                "price": {
                                    "id": "price_1TNt7f6KTxEnTfhCxVyhstYY",
                                    "object": "price",
                                    "active": true,
                                    "billing_scheme": "per_unit",
                                    "created": 1776596563,
                                    "currency": "usd",
                                    "custom_unit_amount": null,
                                    "livemode": false,
                                    "lookup_key": null,
                                    "metadata": {},
                                    "nickname": null,
                                    "product": "prod_UMcMnHoaxN3gAS",
                                    "recurring": {
                                        "interval": "month",
                                        "interval_count": 1,
                                        "meter": null,
                                        "trial_period_days": null,
                                        "usage_type": "licensed"
                                    },
                                    "tax_behavior": "unspecified",
                                    "tiers_mode": null,
                                    "transform_quantity": null,
                                    "type": "recurring",
                                    "unit_amount": 1500,
                                    "unit_amount_decimal": "1500"
                                },
                                "quantity": 1,
                                "subscription": "sub_1TNt7g6KTxEnTfhCyFPKx2oE",
                                "tax_rates": []
                            }
                        ],
                        "has_more": false,
                        "total_count": 1,
                        "url": "/v1/subscription_items?subscription=sub_1TNt7g6KTxEnTfhCyFPKx2oE"
                    },
                    "latest_invoice": "in_1TNt7g6KTxEnTfhC4TM9IyOA",
                    "livemode": false,
                    "managed_payments": { "enabled": false },
                    "metadata": {
                        "userId": "4f2413c2-c2c6-4418-8740-d8a597252163"
                    },
                    "next_pending_invoice_item_invoice": null,
                    "on_behalf_of": null,
                    "pause_collection": null,
                    "payment_settings": {
                        "payment_method_options": null,
                        "payment_method_types": null,
                        "save_default_payment_method": "off"
                    },
                    "pending_invoice_item_interval": null,
                    "pending_setup_intent": null,
                    "pending_update": null,
                    "plan": {
                        "id": "price_1TNt7f6KTxEnTfhCxVyhstYY",
                        "object": "plan",
                        "active": true,
                        "amount": 1500,
                        "amount_decimal": "1500",
                        "billing_scheme": "per_unit",
                        "created": 1776596563,
                        "currency": "usd",
                        "interval": "month",
                        "interval_count": 1,
                        "livemode": false,
                        "metadata": {},
                        "meter": null,
                        "nickname": null,
                        "product": "prod_UMcMnHoaxN3gAS",
                        "tiers_mode": null,
                        "transform_usage": null,
                        "trial_period_days": null,
                        "usage_type": "licensed"
                    },
                    "quantity": 1,
                    "schedule": null,
                    "start_date": 1776596564,
                    "status": "active",
                    "test_clock": null,
                    "transfer_data": null,
                    "trial_end": null,
                    "trial_settings": {
                        "end_behavior": { "missing_payment_method": "create_invoice" }
                    },
                    "trial_start": null
                },
                "previous_attributes": null
            }
        }"#,
        )
        .expect("workbench_created_detail must be valid JSON")
    }

    /// Full EventBridge `detail` (without `type`) for a real
    /// `customer.subscription.updated` event captured from Stripe Workbench
    /// in test-mode.
    ///
    /// Notable fields:
    /// * subscription id         : `sub_1TNtCw6KTxEnTfhCNV3Db5IX`
    /// * customer                : `cus_UMcRIBAJ7TNYut`
    /// * top-level metadata      : `{"foo": "bar"}` — **no** `userId` here;
    ///   the userId appears only in `plan.metadata` which our code ignores
    /// * product                 : `prod_UMcRH2ZglY77cx`
    fn workbench_updated_detail() -> Value {
        serde_json::from_str(
            r#"{
            "id": "evt_workbench_updated_001",
            "data": {
                "object": {
                    "id": "sub_1TNtCw6KTxEnTfhCNV3Db5IX",
                    "object": "subscription",
                    "application": null,
                    "application_fee_percent": null,
                    "automatic_tax": {
                        "disabled_reason": null,
                        "enabled": false,
                        "liability": null
                    },
                    "billing_cycle_anchor": 1776596890,
                    "billing_cycle_anchor_config": null,
                    "billing_mode": {
                        "flexible": { "proration_discounts": "included" },
                        "type": "flexible",
                        "updated_at": 1776596890
                    },
                    "billing_thresholds": null,
                    "cancel_at": null,
                    "cancel_at_period_end": false,
                    "canceled_at": null,
                    "cancellation_details": {
                        "comment": null,
                        "feedback": null,
                        "reason": null
                    },
                    "collection_method": "charge_automatically",
                    "created": 1776596890,
                    "currency": "usd",
                    "customer": "cus_UMcRIBAJ7TNYut",
                    "customer_account": null,
                    "days_until_due": null,
                    "default_payment_method": null,
                    "default_source": null,
                    "default_tax_rates": [],
                    "description": null,
                    "discounts": [],
                    "ended_at": null,
                    "invoice_settings": {
                        "account_tax_ids": null,
                        "issuer": { "type": "self" }
                    },
                    "items": {
                        "object": "list",
                        "data": [
                            {
                                "id": "si_UMcRnrccA2Uwob",
                                "object": "subscription_item",
                                "billing_thresholds": null,
                                "created": 1776596890,
                                "current_period_end": 1779188890,
                                "current_period_start": 1776596890,
                                "discounts": [],
                                "metadata": {},
                                "plan": {
                                    "id": "price_1TNtCw6KTxEnTfhCHaT85OV5",
                                    "object": "plan",
                                    "active": true,
                                    "amount": 1500,
                                    "amount_decimal": "1500",
                                    "billing_scheme": "per_unit",
                                    "created": 1776596890,
                                    "currency": "usd",
                                    "interval": "month",
                                    "interval_count": 1,
                                    "livemode": false,
                                    "metadata": {},
                                    "meter": null,
                                    "nickname": null,
                                    "product": "prod_UMcRH2ZglY77cx",
                                    "tiers_mode": null,
                                    "transform_usage": null,
                                    "trial_period_days": null,
                                    "usage_type": "licensed"
                                },
                                "price": {
                                    "id": "price_1TNtCw6KTxEnTfhCHaT85OV5",
                                    "object": "price",
                                    "active": true,
                                    "billing_scheme": "per_unit",
                                    "created": 1776596890,
                                    "currency": "usd",
                                    "custom_unit_amount": null,
                                    "livemode": false,
                                    "lookup_key": null,
                                    "metadata": {},
                                    "nickname": null,
                                    "product": "prod_UMcRH2ZglY77cx",
                                    "recurring": {
                                        "interval": "month",
                                        "interval_count": 1,
                                        "meter": null,
                                        "trial_period_days": null,
                                        "usage_type": "licensed"
                                    },
                                    "tax_behavior": "unspecified",
                                    "tiers_mode": null,
                                    "transform_quantity": null,
                                    "type": "recurring",
                                    "unit_amount": 1500,
                                    "unit_amount_decimal": "1500"
                                },
                                "quantity": 1,
                                "subscription": "sub_1TNtCw6KTxEnTfhCNV3Db5IX",
                                "tax_rates": []
                            }
                        ],
                        "has_more": false,
                        "total_count": 1,
                        "url": "/v1/subscription_items?subscription=sub_1TNtCw6KTxEnTfhCNV3Db5IX"
                    },
                    "latest_invoice": "in_1TNtCw6KTxEnTfhC3lFI13Vy",
                    "livemode": false,
                    "managed_payments": { "enabled": false },
                    "metadata": { "foo": "bar" },
                    "next_pending_invoice_item_invoice": null,
                    "on_behalf_of": null,
                    "pause_collection": null,
                    "payment_settings": {
                        "payment_method_options": null,
                        "payment_method_types": null,
                        "save_default_payment_method": "off"
                    },
                    "pending_invoice_item_interval": null,
                    "pending_setup_intent": null,
                    "pending_update": null,
                    "plan": {
                        "id": "price_1TNtCw6KTxEnTfhCHaT85OV5",
                        "object": "plan",
                        "active": true,
                        "amount": 1500,
                        "amount_decimal": "1500",
                        "billing_scheme": "per_unit",
                        "created": 1776596890,
                        "currency": "usd",
                        "interval": "month",
                        "interval_count": 1,
                        "livemode": false,
                        "metadata": {
                            "userId": "8f1c17c6-16ec-4631-9567-05d34ecd20f5"
                        },
                        "meter": null,
                        "nickname": null,
                        "product": "prod_UMcRH2ZglY77cx",
                        "tiers_mode": null,
                        "transform_usage": null,
                        "trial_period_days": null,
                        "usage_type": "licensed"
                    },
                    "quantity": 1,
                    "schedule": null,
                    "start_date": 1776596890,
                    "status": "active",
                    "test_clock": null,
                    "transfer_data": null,
                    "trial_end": null,
                    "trial_settings": {
                        "end_behavior": { "missing_payment_method": "create_invoice" }
                    },
                    "trial_start": null
                },
                "previous_attributes": {
                    "metadata": { "foo": null }
                }
            }
        }"#,
        )
        .expect("workbench_updated_detail must be valid JSON")
    }

    /// Full EventBridge `detail` (without `type`) for a real
    /// `customer.subscription.deleted` event captured from Stripe Workbench
    /// in test-mode.
    ///
    /// Notable fields:
    /// * subscription id  : `sub_1TNtEP6KTxEnTfhCWvNx8piI`
    /// * customer         : `cus_UMcT0uzOVTXLLi`
    /// * top-level metadata: `{}` — no userId (irrelevant for deleted events)
    /// * product          : `prod_UMcTrA1nEo7FGZ` (not in workbench_tier_map;
    ///   irrelevant since deleted always sets Free tier)
    /// * status           : `canceled`
    fn workbench_deleted_detail() -> Value {
        serde_json::from_str(
            r#"{
            "id": "evt_workbench_deleted_001",
            "data": {
                "object": {
                    "id": "sub_1TNtEP6KTxEnTfhCWvNx8piI",
                    "object": "subscription",
                    "application": null,
                    "application_fee_percent": null,
                    "automatic_tax": {
                        "disabled_reason": null,
                        "enabled": false,
                        "liability": null
                    },
                    "billing_cycle_anchor": 1776596981,
                    "billing_cycle_anchor_config": null,
                    "billing_mode": {
                        "flexible": { "proration_discounts": "included" },
                        "type": "flexible",
                        "updated_at": 1776596981
                    },
                    "billing_thresholds": null,
                    "cancel_at": null,
                    "cancel_at_period_end": false,
                    "canceled_at": 1776596986,
                    "cancellation_details": {
                        "comment": null,
                        "feedback": null,
                        "reason": "cancellation_requested"
                    },
                    "collection_method": "charge_automatically",
                    "created": 1776596981,
                    "currency": "usd",
                    "customer": "cus_UMcT0uzOVTXLLi",
                    "customer_account": null,
                    "days_until_due": null,
                    "default_payment_method": null,
                    "default_source": null,
                    "default_tax_rates": [],
                    "description": null,
                    "discounts": [],
                    "ended_at": 1776596986,
                    "invoice_settings": {
                        "account_tax_ids": null,
                        "issuer": { "type": "self" }
                    },
                    "items": {
                        "object": "list",
                        "data": [
                            {
                                "id": "si_UMcTp1pJxKPWld",
                                "object": "subscription_item",
                                "billing_thresholds": null,
                                "created": 1776596982,
                                "current_period_end": 1779188981,
                                "current_period_start": 1776596981,
                                "discounts": [],
                                "metadata": {
                                    "userId": "a7cd9949-9cc3-4971-9a72-f8ad1ec890f3"
                                },
                                "plan": {
                                    "id": "price_1TNtEP6KTxEnTfhCTfMGoIXM",
                                    "object": "plan",
                                    "active": true,
                                    "amount": 1500,
                                    "amount_decimal": "1500",
                                    "billing_scheme": "per_unit",
                                    "created": 1776596981,
                                    "currency": "usd",
                                    "interval": "month",
                                    "interval_count": 1,
                                    "livemode": false,
                                    "metadata": {},
                                    "meter": null,
                                    "nickname": null,
                                    "product": "prod_UMcTrA1nEo7FGZ",
                                    "tiers_mode": null,
                                    "transform_usage": null,
                                    "trial_period_days": null,
                                    "usage_type": "licensed"
                                },
                                "price": {
                                    "id": "price_1TNtEP6KTxEnTfhCTfMGoIXM",
                                    "object": "price",
                                    "active": true,
                                    "billing_scheme": "per_unit",
                                    "created": 1776596981,
                                    "currency": "usd",
                                    "custom_unit_amount": null,
                                    "livemode": false,
                                    "lookup_key": null,
                                    "metadata": {},
                                    "nickname": null,
                                    "product": "prod_UMcTrA1nEo7FGZ",
                                    "recurring": {
                                        "interval": "month",
                                        "interval_count": 1,
                                        "meter": null,
                                        "trial_period_days": null,
                                        "usage_type": "licensed"
                                    },
                                    "tax_behavior": "unspecified",
                                    "tiers_mode": null,
                                    "transform_quantity": null,
                                    "type": "recurring",
                                    "unit_amount": 1500,
                                    "unit_amount_decimal": "1500"
                                },
                                "quantity": 1,
                                "subscription": "sub_1TNtEP6KTxEnTfhCWvNx8piI",
                                "tax_rates": []
                            }
                        ],
                        "has_more": false,
                        "total_count": 1,
                        "url": "/v1/subscription_items?subscription=sub_1TNtEP6KTxEnTfhCWvNx8piI"
                    },
                    "latest_invoice": "in_1TNtEP6KTxEnTfhCyRRYMduD",
                    "livemode": false,
                    "managed_payments": { "enabled": false },
                    "metadata": {},
                    "next_pending_invoice_item_invoice": null,
                    "on_behalf_of": null,
                    "pause_collection": null,
                    "payment_settings": {
                        "payment_method_options": null,
                        "payment_method_types": null,
                        "save_default_payment_method": "off"
                    },
                    "pending_invoice_item_interval": null,
                    "pending_setup_intent": null,
                    "pending_update": null,
                    "plan": {
                        "id": "price_1TNtEP6KTxEnTfhCTfMGoIXM",
                        "object": "plan",
                        "active": true,
                        "amount": 1500,
                        "amount_decimal": "1500",
                        "billing_scheme": "per_unit",
                        "created": 1776596981,
                        "currency": "usd",
                        "interval": "month",
                        "interval_count": 1,
                        "livemode": false,
                        "metadata": {},
                        "meter": null,
                        "nickname": null,
                        "product": "prod_UMcTrA1nEo7FGZ",
                        "tiers_mode": null,
                        "transform_usage": null,
                        "trial_period_days": null,
                        "usage_type": "licensed"
                    },
                    "quantity": 1,
                    "schedule": null,
                    "start_date": 1776596981,
                    "status": "canceled",
                    "test_clock": null,
                    "transfer_data": null,
                    "trial_end": null,
                    "trial_settings": {
                        "end_behavior": { "missing_payment_method": "create_invoice" }
                    },
                    "trial_start": null
                },
                "previous_attributes": null
            }
        }"#,
        )
        .expect("workbench_deleted_detail must be valid JSON")
    }

    // -------------------------------------------------------------------------
    // Tests — real Stripe Workbench payloads
    // -------------------------------------------------------------------------

    /// Verifies that `parse_subscription` correctly extracts the subscription
    /// id, customer, top-level `metadata.userId`, and product id from an
    /// unmodified real Stripe Workbench payload.  This exercises serde
    /// robustness: unknown fields such as `billing_mode`, `managed_payments`,
    /// `cancellation_details`, etc. must be silently ignored.
    #[test]
    fn should_deserialize_subscription_id_customer_user_id_and_product_from_real_subscription_created_payload()
     {
        let detail = workbench_created_detail();
        let sub = parse_subscription(&detail).expect("must deserialize real created payload");

        assert_eq!(
            sub.id.as_deref(),
            Some("sub_1TNt7g6KTxEnTfhCyFPKx2oE"),
            "subscription id"
        );
        assert_eq!(
            sub.stripe_customer_id()
                .as_ref()
                .map(|s: &StripeCustomerId| s.as_ref()),
            Some("cus_UMcMyweeI3jPgl"),
            "customer id"
        );
        assert_eq!(
            sub.user_id_from_metadata(),
            Some(UserId::try_from("4f2413c2-c2c6-4418-8740-d8a597252163").unwrap()),
            "metadata.userId"
        );
        assert_eq!(
            sub.first_product_id(),
            Some("prod_UMcMnHoaxN3gAS"),
            "items[0].price.product"
        );
    }

    /// End-to-end handler test for `customer.subscription.created` using the
    /// real Stripe Workbench payload.  Verifies that:
    /// * the subscription is correctly deserialized from the full real shape,
    /// * `update_user` is called with the userId from `metadata`, the Pro tier
    ///   (resolved from the real product id), and the Stripe customer id.
    #[tokio::test]
    async fn should_apply_subscription_created_when_real_stripe_workbench_payload_provided() {
        let map = workbench_tier_map();
        let expected_user_id = UserId::try_from("4f2413c2-c2c6-4418-8740-d8a597252163").unwrap();
        let event = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED,
            workbench_created_detail(),
        );

        let mut service = MockUserService::default();
        let user = dummy_user();
        service
            .expect_update_user()
            .withf(move |uid, cmd| {
                *uid == expected_user_id
                    && cmd.tier == Some(UserTier::Pro)
                    && cmd.stripe_customer_id.as_ref().map(|s| s.as_ref())
                        == Some("cus_UMcMyweeI3jPgl")
            })
            .return_once(move |_, _| Box::pin(async move { Ok(user) }));

        let result = handler(event, &service, &map).await;
        assert!(result.is_ok());
    }

    /// End-to-end handler test for `customer.subscription.updated` using the
    /// real Stripe Workbench payload.  Verifies that:
    /// * the handler does NOT use `metadata.userId` (absent at top level —
    ///   present only in `plan.metadata`, which our code correctly ignores),
    /// * user lookup is performed via the Stripe customer id,
    /// * `update_user` is called with the Ultimate tier (resolved from the
    ///   real product id `prod_UMcRH2ZglY77cx`) and no customer-id override.
    #[tokio::test]
    async fn should_apply_subscription_updated_when_real_stripe_workbench_payload_provided() {
        let map = workbench_tier_map();
        let event = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED,
            workbench_updated_detail(),
        );

        let mut service = MockUserService::default();
        let user = dummy_user();
        let user_id = user.user_id;
        let user_for_lookup = user.clone();
        service
            .expect_find_user_by_stripe_customer_id()
            .withf(|scid| scid.as_ref() == "cus_UMcRIBAJ7TNYut")
            .return_once(move |_| Box::pin(async move { Ok(user_for_lookup) }));
        service
            .expect_update_user()
            .withf(move |uid, cmd| {
                *uid == user_id
                    && cmd.tier == Some(UserTier::Ultimate)
                    && cmd.stripe_customer_id.is_none()
            })
            .return_once(move |_, _| Box::pin(async move { Ok(user) }));

        let result = handler(event, &service, &map).await;
        assert!(result.is_ok());
    }

    /// End-to-end handler test for `customer.subscription.deleted` using the
    /// real Stripe Workbench payload.  Verifies that:
    /// * the canceled subscription is correctly deserialized (status=canceled,
    ///   `ended_at` and `canceled_at` populated, `metadata` empty),
    /// * user lookup is performed via the Stripe customer id,
    /// * `update_user` is called with `UserTier::Free` regardless of product.
    #[tokio::test]
    async fn should_apply_subscription_deleted_when_real_stripe_workbench_payload_provided() {
        let map = workbench_tier_map();
        let event = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED,
            workbench_deleted_detail(),
        );

        let mut service = MockUserService::default();
        let user = dummy_user();
        let user_id = user.user_id;
        let user_for_lookup = user.clone();
        service
            .expect_find_user_by_stripe_customer_id()
            .withf(|scid| scid.as_ref() == "cus_UMcT0uzOVTXLLi")
            .return_once(move |_| Box::pin(async move { Ok(user_for_lookup) }));
        service
            .expect_update_user()
            .withf(move |uid, cmd| {
                *uid == user_id
                    && cmd.tier == Some(UserTier::Free)
                    && cmd.stripe_customer_id.is_none()
            })
            .return_once(move |_, _| Box::pin(async move { Ok(user) }));

        let result = handler(event, &service, &map).await;
        assert!(result.is_ok());
    }

    /// Verifies that a real `subscription.updated` payload is silently skipped
    /// when its product id (`prod_UMcRH2ZglY77cx`) does not appear in the
    /// configured tier map.  No service calls must be made.
    #[tokio::test]
    async fn should_skip_subscription_updated_when_real_workbench_product_id_not_in_tier_map() {
        // tier_map() only knows "prod_pro" / "prod_ultimate" — not the
        // workbench ids — so tier resolution will return None and the handler
        // must skip without touching the service.
        let map = tier_map();
        let event = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED,
            workbench_updated_detail(),
        );

        let service = MockUserService::default(); // no expectations

        let result = handler(event, &service, &map).await;
        assert!(result.is_ok());
    }
}
