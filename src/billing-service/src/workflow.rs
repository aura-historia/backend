//! V1 billing operation state. No provider I/O runs inside a database transaction.
use crate::ports::*;
use crate::use_cases::commands::create_billing_checkout_session::{
    BillingCycle, BillingPlan, BillingPriceIds, billing_service_context, user_name,
};
use application::{
    error::static_error,
    external_operation::{ExternalAttempt, ExternalOperations},
    idempotency::{CommandFingerprint, CommandIntent, CommandReceiptError},
    operation_context::OperationContext,
};
use serde::{Deserialize, Serialize};
use url::Url;
use user_core::{stripe_customer_id::StripeCustomerId, tier::UserTier, user_id::UserId};
use user_service::{
    ports::UserDetailsView,
    use_cases::{
        AssociateUserStripeCustomerIdCommand, AssociateUserStripeCustomerIdUseCase,
        GetOwnUserRequest, GetOwnUserUseCase,
    },
};

#[derive(thiserror::Error, Debug)]
pub(crate) enum WorkflowError {
    #[error(transparent)]
    Receipt(#[from] CommandReceiptError),
    #[error("customer already exists")]
    ExistingCustomer,
    #[error("customer missing")]
    MissingCustomer,
    #[error(transparent)]
    Provider(#[from] StripeBillingError),
    #[error(transparent)]
    Association(#[from] user_service::use_cases::AssociateUserStripeCustomerIdError),
    #[error(transparent)]
    User(#[from] user_service::use_cases::GetOwnUserError),
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StateV1 {
    user_id: UserId,
    email: serde_email::Email,
    name: Option<String>,
    customer: Option<StripeCustomerId>,
    price: Option<String>,
    redirects: Option<CheckoutRedirects>,
    return_url: Option<Url>,
    initialized: bool,
    rejected_existing: bool,
    rejected_missing: bool,
    associated: bool,
    url: Option<Url>,
    url_deadline: Option<i64>,
}
fn invalid(e: impl Into<application::error::BoxError>) -> WorkflowError {
    CommandReceiptError::InvalidPersistedState(e.into()).into()
}
fn state_value(state: &StateV1) -> Result<serde_json::Value, WorkflowError> {
    serde_json::to_value(state).map_err(invalid)
}
fn provider_key(intent: &CommandIntent, step: &str) -> String {
    let value = serde_json::json!([
        intent.actor,
        intent.operation,
        intent.target,
        intent.key.as_str(),
        step
    ]);
    let hash = CommandFingerprint::digest(1, value.to_string().as_bytes());
    hash.as_bytes().iter().map(|v| format!("{v:02x}")).collect()
}
pub(crate) struct Request<'a> {
    pub operation: &'static str,
    pub plan: Option<BillingPlan>,
    pub cycle: Option<BillingCycle>,
    pub prices: Option<&'a BillingPriceIds>,
    pub strict_checkout: bool,
    pub portal_only: bool,
}
#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute<U, A, C, S, P>(
    context: &OperationContext,
    user: UserDetailsView,
    request: Request<'_>,
    operations: &dyn ExternalOperations,
    users: &U,
    associate: &A,
    customers: &C,
    checkout: &S,
    portal: &P,
) -> Result<Url, WorkflowError>
where
    U: GetOwnUserUseCase,
    A: AssociateUserStripeCustomerIdUseCase,
    C: StripeCustomerCreator,
    S: StripeCheckoutSessionCreator,
    P: StripePortalSessionCreator,
{
    let owner = user.user_id;
    let intent=CommandIntent::new(context,request.operation,owner.to_string(),Some(owner),serde_json::json!({"plan":request.plan.map(|v|match v{BillingPlan::Pro=>"PRO",BillingPlan::Ultimate=>"ULTIMATE"}),"cycle":request.cycle.map(|v|match v{BillingCycle::Monthly=>"MONTHLY",BillingCycle::Yearly=>"YEARLY"})})).ok_or_else(||invalid(static_error("missing billing command key")))?;
    let use_portal =
        request.portal_only || (!request.strict_checkout && user.tier != UserTier::Free);
    let initial = StateV1 {
        user_id: owner,
        email: user.email,
        name: user_name(&user.first_name, &user.last_name),
        customer: user.stripe_customer_id,
        price: if use_portal {
            None
        } else {
            Some(
                request
                    .prices
                    .ok_or_else(|| invalid(static_error("missing prices")))?
                    .price_id(
                        request
                            .plan
                            .ok_or_else(|| invalid(static_error("missing plan")))?,
                        request
                            .cycle
                            .ok_or_else(|| invalid(static_error("missing cycle")))?,
                    )
                    .to_owned(),
            )
        },
        redirects: if use_portal {
            None
        } else {
            checkout.redirects()
        },
        return_url: if use_portal {
            portal.return_url()
        } else {
            None
        },
        initialized: false,
        rejected_existing: false,
        rejected_missing: false,
        associated: false,
        url: None,
        url_deadline: None,
    };
    let ExternalAttempt {
        state,
        attempt,
        admitted_at,
        completed,
    } = operations
        .claim(
            &intent,
            state_value(&initial)?,
            &format!("BILLING:{owner}"),
            true,
        )
        .await?;
    let mut state: StateV1 = serde_json::from_value(state).map_err(invalid)?;
    if state.user_id != owner {
        return Err(invalid(static_error("billing operation owner mismatch")));
    }
    if state.rejected_missing {
        return Err(WorkflowError::MissingCustomer);
    }
    if state.rejected_existing {
        return Err(WorkflowError::ExistingCustomer);
    }
    if completed {
        if state
            .url_deadline
            .is_none_or(|deadline| deadline <= time::OffsetDateTime::now_utc().unix_timestamp())
        {
            return Err(CommandReceiptError::ResultUnavailable.into());
        }
        return state
            .url
            .ok_or_else(|| invalid(static_error("missing completed billing URL")));
    }
    // Provider API v1 keys may be pruned after 24h. Never retry ambiguous work beyond 23h.
    if time::OffsetDateTime::now_utc() - admitted_at >= time::Duration::hours(23) {
        return Err(CommandReceiptError::ResultUnavailable.into());
    }
    if !state.initialized {
        // Re-read after durable scope admission so different keys cannot race a stale customer guard.
        let current = users.execute(context, GetOwnUserRequest).await?;
        if request.strict_checkout && current.stripe_customer_id.is_some() {
            state.rejected_existing = true;
            operations
                .checkpoint(&intent, attempt, state_value(&state)?, true)
                .await?;
            return Err(WorkflowError::ExistingCustomer);
        }
        state.customer = current.stripe_customer_id;
        state.initialized = true;
        operations
            .checkpoint(&intent, attempt, state_value(&state)?, false)
            .await?;
    }
    if state.price.is_none() && state.customer.is_none() {
        state.rejected_missing = true;
        operations
            .checkpoint(&intent, attempt, state_value(&state)?, true)
            .await?;
        return Err(WorkflowError::MissingCustomer);
    }
    let customer = match state.customer.clone() {
        Some(id) => id,
        None => {
            let id = customers
                .create_customer(CreateStripeCustomerRequest {
                    user_id: owner,
                    email: state.email.clone(),
                    name: state.name.clone(),
                    idempotency_key: Some(provider_key(&intent, "CUSTOMER_V1")),
                })
                .await?;
            state.customer = Some(id.clone());
            operations
                .checkpoint(&intent, attempt, state_value(&state)?, false)
                .await?;
            id
        }
    };
    if !state.associated {
        associate
            .execute(
                &billing_service_context(context),
                AssociateUserStripeCustomerIdCommand {
                    user_id: owner,
                    stripe_customer_id: customer.clone(),
                },
            )
            .await?;
        state.associated = true;
        operations
            .checkpoint(&intent, attempt, state_value(&state)?, false)
            .await?;
    }
    let url = if let Some(price) = state.price.clone() {
        if state.redirects.is_none() {
            return Err(invalid(static_error("missing frozen checkout redirects")));
        }
        checkout
            .create_checkout_session(CreateStripeCheckoutSessionRequest {
                redirects: state.redirects.clone(),
                user_id: owner,
                stripe_customer_id: customer,
                price_id: price,
                idempotency_key: Some(provider_key(&intent, "CHECKOUT_V1")),
            })
            .await?
    } else {
        if state.return_url.is_none() {
            return Err(invalid(static_error("missing frozen portal redirect")));
        }
        portal
            .create_portal_session(CreateStripePortalSessionRequest {
                return_url: state.return_url.clone(),
                stripe_customer_id: customer,
                idempotency_key: Some(provider_key(&intent, "PORTAL_V1")),
            })
            .await?
    };
    // Capability URLs have a shorter recovery window than the durable execution fence.
    state.url = Some(url.clone());
    state.url_deadline = Some(
        (admitted_at
            + if state.price.is_some() {
                time::Duration::hours(23)
            } else {
                time::Duration::minutes(5)
            })
        .unix_timestamp(),
    );
    operations
        .checkpoint(&intent, attempt, state_value(&state)?, true)
        .await?;
    Ok(url)
}

/// Unused capability for checkout-only or portal-only compositions.
pub(crate) struct Unused;
#[async_trait::async_trait]
impl StripePortalSessionCreator for Unused {
    async fn create_portal_session(
        &self,
        _: CreateStripePortalSessionRequest,
    ) -> Result<Url, StripeBillingError> {
        Err(StripeBillingError::InvalidResponse {
            source: static_error("unexpected portal operation"),
        })
    }
}
pub(crate) async fn portal<P: StripePortalSessionCreator>(
    context: &OperationContext,
    user: UserDetailsView,
    operations: &dyn ExternalOperations,
    portal: &P,
) -> Result<Url, WorkflowError> {
    let owner = user.user_id;
    let intent = CommandIntent::new(
        context,
        "CREATE_BILLING_PORTAL",
        owner.to_string(),
        Some(owner),
        serde_json::Value::Null,
    )
    .ok_or_else(|| invalid(static_error("missing portal key")))?;
    let initial = StateV1 {
        user_id: owner,
        email: user.email,
        name: None,
        customer: user.stripe_customer_id,
        price: None,
        redirects: None,
        return_url: portal.return_url(),
        initialized: true,
        rejected_existing: false,
        rejected_missing: false,
        associated: true,
        url: None,
        url_deadline: None,
    };
    let operation = operations
        .claim(
            &intent,
            state_value(&initial)?,
            &format!("BILLING:{owner}"),
            true,
        )
        .await?;
    let mut state: StateV1 = serde_json::from_value(operation.state).map_err(invalid)?;
    if state.user_id != owner {
        return Err(invalid(static_error("portal owner mismatch")));
    }
    if state.rejected_missing {
        return Err(WorkflowError::MissingCustomer);
    }
    if operation.completed {
        if state
            .url_deadline
            .is_none_or(|d| d <= time::OffsetDateTime::now_utc().unix_timestamp())
        {
            return Err(CommandReceiptError::ResultUnavailable.into());
        }
        return state
            .url
            .ok_or_else(|| invalid(static_error("missing portal URL")));
    }
    if time::OffsetDateTime::now_utc() - operation.admitted_at >= time::Duration::hours(23) {
        return Err(CommandReceiptError::ResultUnavailable.into());
    }
    let customer = match state.customer.clone() {
        Some(customer) => customer,
        None => {
            state.rejected_missing = true;
            operations
                .checkpoint(&intent, operation.attempt, state_value(&state)?, true)
                .await?;
            return Err(WorkflowError::MissingCustomer);
        }
    };
    let return_url = state
        .return_url
        .clone()
        .ok_or_else(|| invalid(static_error("missing frozen return URL")))?;
    let url = portal
        .create_portal_session(CreateStripePortalSessionRequest {
            return_url: Some(return_url),
            stripe_customer_id: customer,
            idempotency_key: Some(provider_key(&intent, "PORTAL_V1")),
        })
        .await?;
    state.url = Some(url.clone());
    state.url_deadline =
        Some((operation.admitted_at + time::Duration::minutes(5)).unix_timestamp());
    operations
        .checkpoint(&intent, operation.attempt, state_value(&state)?, true)
        .await?;
    Ok(url)
}
