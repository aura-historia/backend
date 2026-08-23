//! AWS Lambda edge handler for Stripe subscription lifecycle events.
//!
//! Stripe sends events through EventBridge. This crate validates and maps the
//! envelope, then invokes the canonical User service use case. PostgreSQL
//! transactions, user locking, persistence, and entitlement reconciliation stay
//! inside `user-service` and its adapters.

use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use aws_lambda_events::eventbridge::EventBridgeEvent;
use lambda_runtime::LambdaEvent;
use serde::Deserialize;
use serde_json::Value;
use tracing::{error, info, warn};
use user_core::tier::UserTier;
use user_core::{stripe_customer_id::StripeCustomerId, user_id::UserId};
use user_service::use_cases::{
    ApplyStripeSubscriptionCommand, ApplyStripeSubscriptionError, ApplyStripeSubscriptionTarget,
    ApplyStripeSubscriptionUseCase,
};

pub const STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED: &str = "customer.subscription.created";
pub const STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED: &str = "customer.subscription.updated";
pub const STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED: &str = "customer.subscription.deleted";

#[derive(Debug, Clone)]
pub struct StripeProductTierMap {
    pub pro_product_id: String,
    pub ultimate_product_id: String,
}

impl StripeProductTierMap {
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

#[derive(Debug, Clone, Deserialize)]
pub struct StripeSubscription {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub customer: Option<String>,
    #[serde(default)]
    pub metadata: std::collections::BTreeMap<String, String>,
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
    #[serde(default)]
    pub product: Option<String>,
}

impl StripeSubscription {
    pub fn first_product_id(&self) -> Option<&str> {
        self.items
            .as_ref()
            .and_then(|items| items.data.first())
            .and_then(|item| item.price.as_ref())
            .and_then(|price| price.product.as_deref())
    }

    pub fn user_id_from_metadata(&self) -> Option<UserId> {
        self.metadata
            .get("userId")
            .and_then(|value| UserId::try_from(value.as_str()).ok())
    }

    pub fn stripe_customer_id(&self) -> Option<StripeCustomerId> {
        self.customer
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(StripeCustomerId::from)
    }
}

#[tracing::instrument(
    skip(event, subscriptions, tier_map),
    fields(
        request_id = %event.context.request_id,
        event_bridge_event_id = tracing::field::Empty,
        stripe_event_id = tracing::field::Empty,
        stripe_type = tracing::field::Empty,
        source = %event.payload.source,
    )
)]
pub async fn handler(
    event: LambdaEvent<EventBridgeEvent<Value>>,
    subscriptions: &(dyn ApplyStripeSubscriptionUseCase + Send + Sync),
    tier_map: &StripeProductTierMap,
) -> Result<(), lambda_runtime::Error> {
    let context = operation_context(&event);
    let payload = event.payload;
    let span = tracing::Span::current();

    if let Some(event_bridge_event_id) = payload.id.as_deref() {
        span.record("event_bridge_event_id", event_bridge_event_id);
    }
    if let Some(stripe_event_id) = payload.detail.get("id").and_then(Value::as_str) {
        span.record("stripe_event_id", stripe_event_id);
    }

    let Some(stripe_type) = stripe_event_type(&payload.detail) else {
        warn!("Stripe event is missing type; ignoring");
        return Ok(());
    };
    span.record("stripe_type", stripe_type);

    match stripe_type {
        STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED => {
            handle_subscription_created(&context, &payload.detail, subscriptions, tier_map).await
        }
        STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED => {
            handle_subscription_updated(&context, &payload.detail, subscriptions, tier_map).await
        }
        STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED => {
            handle_subscription_deleted(&context, &payload.detail, subscriptions).await
        }
        _ => {
            warn!("Received unsupported Stripe event type; ignoring");
            Ok(())
        }
    }
}

fn operation_context(event: &LambdaEvent<EventBridgeEvent<Value>>) -> OperationContext {
    let request_id = RequestId::new(event.context.request_id.clone());
    let correlation_id = event
        .payload
        .id
        .as_deref()
        .map(CorrelationId::new)
        .unwrap_or_else(|| CorrelationId::new(request_id.as_str()));

    OperationContext {
        principal: Principal::System,
        request_id,
        correlation_id,
    }
}

fn stripe_event_type(detail: &Value) -> Option<&str> {
    detail.get("type").and_then(Value::as_str)
}

fn parse_subscription(detail: &Value) -> Option<StripeSubscription> {
    let candidate = detail
        .get("data")
        .and_then(|data| data.get("object"))
        .unwrap_or(detail);

    match serde_json::from_value(candidate.clone()) {
        Ok(subscription) => Some(subscription),
        Err(error) => {
            error!(%error, "Failed to deserialize Stripe subscription; ignoring");
            None
        }
    }
}

async fn handle_subscription_created(
    context: &OperationContext,
    detail: &Value,
    subscriptions: &(dyn ApplyStripeSubscriptionUseCase + Send + Sync),
    tier_map: &StripeProductTierMap,
) -> Result<(), lambda_runtime::Error> {
    let Some(subscription) = parse_subscription(detail) else {
        return Ok(());
    };
    let subscription_id = subscription.id.as_deref().unwrap_or("<unknown>");
    let Some(user_id) = subscription.user_id_from_metadata() else {
        error!(
            subscription_id,
            "Stripe subscription creation has no valid user metadata; ignoring"
        );
        return Ok(());
    };
    let Some(stripe_customer_id) = subscription.stripe_customer_id() else {
        error!(
            subscription_id,
            "Stripe subscription creation has no customer; ignoring"
        );
        return Ok(());
    };
    let Some(product_id) = subscription.first_product_id() else {
        error!(
            subscription_id,
            "Stripe subscription creation has no product; ignoring"
        );
        return Ok(());
    };
    let Some(tier) = tier_map.tier_for(product_id) else {
        warn!(
            subscription_id,
            "Stripe subscription creation has unknown product; ignoring"
        );
        return Ok(());
    };

    apply_subscription(
        context,
        subscriptions,
        ApplyStripeSubscriptionCommand {
            target: ApplyStripeSubscriptionTarget::User(user_id),
            tier,
            associate_stripe_customer_id: Some(stripe_customer_id),
        },
        subscription_id,
        "created",
    )
    .await
}

async fn handle_subscription_updated(
    context: &OperationContext,
    detail: &Value,
    subscriptions: &(dyn ApplyStripeSubscriptionUseCase + Send + Sync),
    tier_map: &StripeProductTierMap,
) -> Result<(), lambda_runtime::Error> {
    let Some(subscription) = parse_subscription(detail) else {
        return Ok(());
    };
    let subscription_id = subscription.id.as_deref().unwrap_or("<unknown>");
    let Some(stripe_customer_id) = subscription.stripe_customer_id() else {
        error!(
            subscription_id,
            "Stripe subscription update has no customer; ignoring"
        );
        return Ok(());
    };
    let Some(product_id) = subscription.first_product_id() else {
        error!(
            subscription_id,
            "Stripe subscription update has no product; ignoring"
        );
        return Ok(());
    };
    let Some(tier) = tier_map.tier_for(product_id) else {
        warn!(
            subscription_id,
            "Stripe subscription update has unknown product; ignoring"
        );
        return Ok(());
    };

    apply_subscription(
        context,
        subscriptions,
        ApplyStripeSubscriptionCommand {
            target: ApplyStripeSubscriptionTarget::StripeCustomer(stripe_customer_id),
            tier,
            associate_stripe_customer_id: None,
        },
        subscription_id,
        "updated",
    )
    .await
}

async fn handle_subscription_deleted(
    context: &OperationContext,
    detail: &Value,
    subscriptions: &(dyn ApplyStripeSubscriptionUseCase + Send + Sync),
) -> Result<(), lambda_runtime::Error> {
    let Some(subscription) = parse_subscription(detail) else {
        return Ok(());
    };
    let subscription_id = subscription.id.as_deref().unwrap_or("<unknown>");
    let Some(stripe_customer_id) = subscription.stripe_customer_id() else {
        error!(
            subscription_id,
            "Stripe subscription deletion has no customer; ignoring"
        );
        return Ok(());
    };

    apply_subscription(
        context,
        subscriptions,
        ApplyStripeSubscriptionCommand {
            target: ApplyStripeSubscriptionTarget::StripeCustomer(stripe_customer_id),
            tier: UserTier::Free,
            associate_stripe_customer_id: None,
        },
        subscription_id,
        "deleted",
    )
    .await
}

async fn apply_subscription(
    context: &OperationContext,
    subscriptions: &(dyn ApplyStripeSubscriptionUseCase + Send + Sync),
    command: ApplyStripeSubscriptionCommand,
    subscription_id: &str,
    event_kind: &str,
) -> Result<(), lambda_runtime::Error> {
    match subscriptions.execute(context, command).await {
        Ok(_) => {
            info!(subscription_id, event_kind, "Stripe subscription processed");
            Ok(())
        }
        Err(ApplyStripeSubscriptionError::UserNotFound) => {
            warn!(
                subscription_id,
                event_kind, "Stripe subscription references unknown user; ignoring"
            );
            Ok(())
        }
        Err(error) => {
            error!(%error, subscription_id, event_kind, "Stripe subscription processing failed");
            Err(Box::new(error))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lambda_runtime::Context;
    use serde_json::json;
    use std::sync::{Arc, Mutex, MutexGuard};

    #[derive(Clone, Default)]
    struct FakeSubscriptions {
        state: Arc<Mutex<FakeSubscriptionsState>>,
    }

    #[derive(Default)]
    struct FakeSubscriptionsState {
        commands: Vec<ApplyStripeSubscriptionCommand>,
        error: Option<FakeError>,
    }

    #[derive(Clone, Copy)]
    enum FakeError {
        UserNotFound,
        Internal,
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[async_trait::async_trait]
    impl ApplyStripeSubscriptionUseCase for FakeSubscriptions {
        async fn execute(
            &self,
            _context: &OperationContext,
            command: ApplyStripeSubscriptionCommand,
        ) -> Result<
            user_service::use_cases::ApplyStripeSubscriptionResult,
            ApplyStripeSubscriptionError,
        > {
            let mut state = lock(&self.state);
            state.commands.push(command);
            match state.error {
                Some(FakeError::UserNotFound) => Err(ApplyStripeSubscriptionError::UserNotFound),
                Some(FakeError::Internal) => {
                    Err(ApplyStripeSubscriptionError::BeginTransactionFailed)
                }
                None => Err(ApplyStripeSubscriptionError::UserNotFound),
            }
        }
    }

    fn tier_map() -> StripeProductTierMap {
        StripeProductTierMap {
            pro_product_id: "prod_pro".to_owned(),
            ultimate_product_id: "prod_ultimate".to_owned(),
        }
    }

    fn lambda_event(event_type: &str, detail: Value) -> LambdaEvent<EventBridgeEvent<Value>> {
        let mut detail = detail;
        if let Some(object) = detail.as_object_mut() {
            object.insert("type".to_owned(), json!(event_type));
        }
        let mut payload = EventBridgeEvent::default();
        payload.id = Some("event-1".to_owned());
        payload.source = "aws.partner/stripe.com/test".to_owned();
        payload.detail = detail;
        LambdaEvent::new(payload, Context::default())
    }

    #[test]
    fn should_map_known_stripe_products_to_tiers() {
        let map = tier_map();
        assert_eq!(Some(UserTier::Pro), map.tier_for("prod_pro"));
        assert_eq!(Some(UserTier::Ultimate), map.tier_for("prod_ultimate"));
        assert_eq!(None, map.tier_for("unknown"));
    }

    #[test]
    fn should_extract_subscription_values() {
        let user_id = UserId::new();
        let subscription: StripeSubscription = serde_json::from_value(json!({
            "customer": "cus_1",
            "metadata": {"userId": user_id.to_string()},
            "items": {"data": [{"price": {"product": "prod_pro"}}]}
        }))
        .unwrap_or_else(|error| panic!("test subscription must deserialize: {error}"));

        assert_eq!(Some(user_id), subscription.user_id_from_metadata());
        assert_eq!(Some("cus_1"), subscription.stripe_customer_id().as_deref());
        assert_eq!(Some("prod_pro"), subscription.first_product_id());
    }

    #[tokio::test]
    async fn should_apply_creation_by_user_and_associate_customer() {
        let user_id = UserId::new();
        let service = FakeSubscriptions::default();
        let event = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED,
            json!({"data": {"object": {
                "id": "sub_1", "customer": "cus_1", "metadata": {"userId": user_id.to_string()},
                "items": {"data": [{"price": {"product": "prod_pro"}}]}
            }}}),
        );

        assert!(handler(event, &service, &tier_map()).await.is_ok());
        assert!(matches!(
            lock(&service.state).commands.as_slice(),
            [ApplyStripeSubscriptionCommand {
                target: ApplyStripeSubscriptionTarget::User(id),
                tier: UserTier::Pro,
                associate_stripe_customer_id: Some(customer),
            }] if *id == user_id && customer.as_ref() == "cus_1"
        ));
    }

    #[tokio::test]
    async fn should_apply_update_and_deletion_by_customer() {
        let service = FakeSubscriptions::default();
        let updated = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED,
            json!({"data": {"object": {
                "customer": "cus_1", "items": {"data": [{"price": {"product": "prod_ultimate"}}]}
            }}}),
        );
        let deleted = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED,
            json!({"data": {"object": {"customer": "cus_1"}}}),
        );

        assert!(handler(updated, &service, &tier_map()).await.is_ok());
        assert!(handler(deleted, &service, &tier_map()).await.is_ok());
        assert!(matches!(
            lock(&service.state).commands.as_slice(),
            [
                ApplyStripeSubscriptionCommand {
                    target: ApplyStripeSubscriptionTarget::StripeCustomer(customer),
                    tier: UserTier::Ultimate,
                    associate_stripe_customer_id: None,
                },
                ApplyStripeSubscriptionCommand {
                    target: ApplyStripeSubscriptionTarget::StripeCustomer(customer_free),
                    tier: UserTier::Free,
                    associate_stripe_customer_id: None,
                },
            ] if customer.as_ref() == "cus_1" && customer_free.as_ref() == "cus_1"
        ));
    }

    #[tokio::test]
    async fn should_skip_invalid_unsupported_and_unknown_user_events() {
        let service = FakeSubscriptions::default();
        lock(&service.state).error = Some(FakeError::UserNotFound);
        let invalid_creation = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED,
            json!({"data": {"object": {"customer": "cus_1"}}}),
        );
        let unsupported = lambda_event("invoice.paid", json!({}));
        let unknown_user = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED,
            json!({"data": {"object": {"customer": "cus_1"}}}),
        );

        assert!(
            handler(invalid_creation, &service, &tier_map())
                .await
                .is_ok()
        );
        assert!(handler(unsupported, &service, &tier_map()).await.is_ok());
        assert!(handler(unknown_user, &service, &tier_map()).await.is_ok());
        assert_eq!(1, lock(&service.state).commands.len());
    }

    #[tokio::test]
    async fn should_fail_for_retry_when_user_service_fails() {
        let service = FakeSubscriptions::default();
        lock(&service.state).error = Some(FakeError::Internal);
        let event = lambda_event(
            STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED,
            json!({"data": {"object": {"customer": "cus_1"}}}),
        );

        assert!(handler(event, &service, &tier_map()).await.is_err());
    }
}
