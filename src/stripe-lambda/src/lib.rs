use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use aws_lambda_events::eventbridge::EventBridgeEvent;
use lambda_runtime::LambdaEvent;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use user_core::stripe_customer_id::StripeCustomerId;
use user_service::use_cases::{ApplyStripeSubscriptionCommand, ApplyStripeSubscriptionUseCase};

pub const STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED: &str = "customer.subscription.created";
pub const STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED: &str = "customer.subscription.updated";
pub const STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED: &str = "customer.subscription.deleted";

#[derive(Deserialize)]
struct StripeEvent {
    id: String,
    data: StripeData,
}
#[derive(Deserialize)]
struct StripeData {
    object: StripeSubscription,
}
#[derive(Deserialize)]
struct StripeSubscription {
    id: String,
    customer: StripeCustomer,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum StripeCustomer {
    Id(String),
    Expanded { id: String },
}

#[tracing::instrument(skip_all, fields(request_id = %event.context.request_id))]
pub async fn handler(
    event: LambdaEvent<EventBridgeEvent<Value>>,
    subscriptions: &(dyn ApplyStripeSubscriptionUseCase + Send + Sync),
) -> Result<(), lambda_runtime::Error> {
    let context = OperationContext {
        principal: Principal::System,
        request_id: RequestId::new(event.context.request_id.clone()),
        correlation_id: CorrelationId::new(
            event.payload.id.clone().unwrap_or(event.context.request_id),
        ),
    };
    let mut detail = event.payload.detail;
    let event_type = detail
        .get("type")
        .and_then(Value::as_str)
        .ok_or("Stripe event has no type")?;
    if !matches!(
        event_type,
        STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED
            | STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED
            | STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED
    ) {
        return Ok(());
    }
    detail.sort_all_objects();
    let fingerprint = Sha256::digest(serde_json::to_vec(&detail)?).into();
    let parsed: StripeEvent =
        serde_json::from_value(detail).map_err(|_| "Stripe subscription event is malformed")?;
    let customer = match parsed.data.object.customer {
        StripeCustomer::Id(id) | StripeCustomer::Expanded { id } => id,
    };
    subscriptions
        .execute(
            &context,
            ApplyStripeSubscriptionCommand {
                event_id: parsed.id,
                subscription_id: parsed.data.object.id,
                customer_id: StripeCustomerId::from(customer),
                fingerprint,
            },
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lambda_runtime::Context;
    use serde_json::json;
    use std::sync::Mutex;
    use user_service::use_cases::{ApplyStripeSubscriptionError, ApplyStripeSubscriptionResult};

    #[derive(Default)]
    struct FakeSubscriptions {
        commands: Mutex<Vec<ApplyStripeSubscriptionCommand>>,
        fail: bool,
    }
    #[async_trait::async_trait]
    impl ApplyStripeSubscriptionUseCase for FakeSubscriptions {
        async fn execute(
            &self,
            _: &OperationContext,
            command: ApplyStripeSubscriptionCommand,
        ) -> Result<ApplyStripeSubscriptionResult, ApplyStripeSubscriptionError> {
            self.commands.lock().unwrap().push(command);
            if self.fail {
                Err(ApplyStripeSubscriptionError::UserNotFound)
            } else {
                Ok(ApplyStripeSubscriptionResult::Applied)
            }
        }
    }
    fn event(kind: &str, data: Value) -> LambdaEvent<EventBridgeEvent<Value>> {
        let mut envelope = EventBridgeEvent::default();
        envelope.detail = json!({"id": "evt_1", "type": kind, "data": {"object": data}});
        LambdaEvent::new(envelope, Context::default())
    }
    #[tokio::test]
    async fn forwards_all_subscription_events_as_reconciliation_requests() {
        let service = FakeSubscriptions::default();
        for kind in [
            STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED,
            STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED,
            STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED,
        ] {
            handler(
                event(kind, json!({"id": "sub_1", "customer": "cus_1"})),
                &service,
            )
            .await
            .unwrap();
        }
        let commands = service.commands.lock().unwrap();
        assert_eq!(3, commands.len());
        assert!(commands.iter().all(|command| command.event_id == "evt_1"
            && command.subscription_id == "sub_1"
            && command.customer_id.as_ref() == "cus_1"));
        assert_ne!(commands[0].fingerprint, commands[1].fingerprint);
    }
    #[tokio::test]
    async fn rejects_malformed_supported_events_and_retries_unknown_users() {
        let service = FakeSubscriptions {
            fail: true,
            ..Default::default()
        };
        assert!(
            handler(
                event(STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED, json!({})),
                &service
            )
            .await
            .is_err()
        );
        assert!(
            handler(
                event(
                    STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED,
                    json!({"id": "sub_1", "customer": {"id": "cus_1"}})
                ),
                &service
            )
            .await
            .is_err()
        );
        assert_eq!(1, service.commands.lock().unwrap().len());
    }
    #[tokio::test]
    async fn ignores_unrelated_events() {
        let service = FakeSubscriptions::default();
        handler(event("invoice.paid", json!({})), &service)
            .await
            .unwrap();
        assert!(service.commands.lock().unwrap().is_empty());
    }
}
