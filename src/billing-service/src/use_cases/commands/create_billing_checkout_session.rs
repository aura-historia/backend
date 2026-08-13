use crate::ports::{
    CreateStripeCheckoutSessionRequest, CreateStripeCustomerRequest, StripeBillingError,
    StripeCheckoutSessionCreator, StripeCustomerCreator,
};
use common::error::boxed::BoxError;
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};

use url::Url;
use user_service::use_cases::{
    AssociateUserStripeCustomerIdCommand, AssociateUserStripeCustomerIdError,
    AssociateUserStripeCustomerIdUseCase, GetOwnUserError, GetOwnUserRequest, GetOwnUserUseCase,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillingPlan {
    Pro,
    Ultimate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillingCycle {
    Monthly,
    Yearly,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateBillingCheckoutSessionCommand {
    pub plan: BillingPlan,
    pub cycle: BillingCycle,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BillingSessionResult {
    pub url: Url,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateBillingCheckoutSessionError {
    #[error("authenticated actor required to create a billing checkout session")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("user not found")]
    UserNotFound,
    #[error("Stripe customer already exists")]
    StripeCustomerAlreadyExists,
    #[error("a different Stripe customer is already associated")]
    StripeCustomerAssociationConflict,
    #[error("concurrent user update")]
    ConcurrencyConflict,
    #[error("Stripe is temporarily unavailable")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("billing provider rejected the request")]
    ProviderRejected {
        #[source]
        source: BoxError,
    },
    #[error("billing provider returned an invalid response")]
    ProviderInvalidResponse {
        #[source]
        source: BoxError,
    },
    #[error("user persistence failed")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait CreateBillingCheckoutSessionUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateBillingCheckoutSessionCommand,
    ) -> Result<BillingSessionResult, CreateBillingCheckoutSessionError>;
}

pub struct CreateBillingCheckoutSessionHandler<U, A, C, S> {
    users: U,
    associate_customer: A,
    customers: C,
    checkout_sessions: S,
    prices: BillingPriceIds,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BillingPriceIds {
    pub pro_monthly: String,
    pub pro_yearly: String,
    pub ultimate_monthly: String,
    pub ultimate_yearly: String,
}

impl BillingPriceIds {
    pub fn price_id(&self, plan: BillingPlan, cycle: BillingCycle) -> &str {
        match (plan, cycle) {
            (BillingPlan::Pro, BillingCycle::Monthly) => &self.pro_monthly,
            (BillingPlan::Pro, BillingCycle::Yearly) => &self.pro_yearly,
            (BillingPlan::Ultimate, BillingCycle::Monthly) => &self.ultimate_monthly,
            (BillingPlan::Ultimate, BillingCycle::Yearly) => &self.ultimate_yearly,
        }
    }
}

impl<U, A, C, S> CreateBillingCheckoutSessionHandler<U, A, C, S> {
    pub fn new(
        users: U,
        associate_customer: A,
        customers: C,
        checkout_sessions: S,
        prices: BillingPriceIds,
    ) -> Self {
        Self {
            users,
            associate_customer,
            customers,
            checkout_sessions,
            prices,
        }
    }
}

#[async_trait::async_trait]
impl<U, A, C, S> CreateBillingCheckoutSessionUseCase
    for CreateBillingCheckoutSessionHandler<U, A, C, S>
where
    U: GetOwnUserUseCase,
    A: AssociateUserStripeCustomerIdUseCase,
    C: StripeCustomerCreator,
    S: StripeCheckoutSessionCreator,
{
    #[tracing::instrument(
        name = "create_billing_checkout_session",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
            outcome = tracing::field::Empty,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateBillingCheckoutSessionCommand,
    ) -> Result<BillingSessionResult, CreateBillingCheckoutSessionError> {
        let user_id = authorize_billing_user(context)?;
        tracing::Span::current().record("actor_id", tracing::field::display(user_id));
        let user = self.users.execute(context, GetOwnUserRequest).await?;
        if user.stripe_customer_id.is_some() {
            return Err(CreateBillingCheckoutSessionError::StripeCustomerAlreadyExists);
        }

        let stripe_customer_id = self
            .customers
            .create_customer(CreateStripeCustomerRequest {
                user_id,
                email: user.email,
                name: user_name(&user.first_name, &user.last_name),
                idempotency_key: command.idempotency_key.clone(),
            })
            .await?;
        self.associate_customer
            .execute(
                &billing_service_context(context),
                AssociateUserStripeCustomerIdCommand {
                    user_id,
                    stripe_customer_id: stripe_customer_id.clone(),
                },
            )
            .await?;
        let url = self
            .checkout_sessions
            .create_checkout_session(CreateStripeCheckoutSessionRequest {
                user_id,
                stripe_customer_id,
                price_id: self.prices.price_id(command.plan, command.cycle).to_owned(),
                idempotency_key: command.idempotency_key,
            })
            .await?;
        tracing::Span::current().record("outcome", "success");
        Ok(BillingSessionResult { url })
    }
}

pub(crate) fn authorize_billing_user(
    context: &OperationContext,
) -> Result<common::user_id::UserId, CreateBillingCheckoutSessionError> {
    context
        .require()
        .credential_capability(CredentialCapability::UsersRead)
        .any_user()
        .authorize::<CreateBillingCheckoutSessionError>()?;
    match context.principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Ok(user_id),
        Principal::Anonymous => Err(CreateBillingCheckoutSessionError::AuthenticatedActorRequired),
        Principal::Service(_) | Principal::System => {
            Err(CreateBillingCheckoutSessionError::Forbidden)
        }
    }
}

pub(crate) fn billing_service_context(context: &OperationContext) -> OperationContext {
    OperationContext {
        principal: Principal::Service("billing-service".to_owned()),
        request_id: context.request_id.clone(),
        correlation_id: context.correlation_id.clone(),
    }
}

pub(crate) fn user_name(
    first_name: &Option<user_core::first_name::FirstName>,
    last_name: &Option<user_core::last_name::LastName>,
) -> Option<String> {
    match (first_name, last_name) {
        (Some(first), Some(last)) => Some(format!("{first} {last}")),
        (Some(first), None) => Some(first.to_string()),
        (None, Some(last)) => Some(last.to_string()),
        (None, None) => None,
    }
}

impl From<OperationAuthorizationError> for CreateBillingCheckoutSessionError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                Self::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}

impl From<GetOwnUserError> for CreateBillingCheckoutSessionError {
    fn from(error: GetOwnUserError) -> Self {
        match error {
            GetOwnUserError::AuthenticatedActorRequired => Self::AuthenticatedActorRequired,
            GetOwnUserError::Forbidden => Self::Forbidden,
            GetOwnUserError::NotFound => Self::UserNotFound,
            GetOwnUserError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            GetOwnUserError::Internal { source } | GetOwnUserError::InvalidReadModel { source } => {
                Self::Internal { source }
            }
            GetOwnUserError::BeginTransactionFailed | GetOwnUserError::CommitTransactionFailed => {
                Self::TemporarilyUnavailable {
                    source: Box::new(error),
                }
            }
        }
    }
}

impl From<AssociateUserStripeCustomerIdError> for CreateBillingCheckoutSessionError {
    fn from(error: AssociateUserStripeCustomerIdError) -> Self {
        match error {
            AssociateUserStripeCustomerIdError::UserNotFound => Self::UserNotFound,
            AssociateUserStripeCustomerIdError::DifferentStripeCustomerAlreadyAssociated
            | AssociateUserStripeCustomerIdError::StripeCustomerConflict { .. } => {
                Self::StripeCustomerAssociationConflict
            }
            AssociateUserStripeCustomerIdError::ConcurrencyConflict => Self::ConcurrencyConflict,
            AssociateUserStripeCustomerIdError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AssociateUserStripeCustomerIdError::BeginTransactionFailed { source }
            | AssociateUserStripeCustomerIdError::CommitTransactionFailed { source } => {
                Self::TemporarilyUnavailable {
                    source: Box::new(source),
                }
            }
            AssociateUserStripeCustomerIdError::InvalidPersistedState { source }
            | AssociateUserStripeCustomerIdError::Internal { source } => Self::Internal { source },
            AssociateUserStripeCustomerIdError::AuthenticatedActorRequired
            | AssociateUserStripeCustomerIdError::Forbidden => Self::Internal {
                source: Box::new(error),
            },
        }
    }
}

impl From<StripeBillingError> for CreateBillingCheckoutSessionError {
    fn from(error: StripeBillingError) -> Self {
        match error {
            StripeBillingError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            StripeBillingError::Rejected { source } => Self::ProviderRejected { source },
            StripeBillingError::InvalidResponse { source } => {
                Self::ProviderInvalidResponse { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::stripe_customer_id::StripeCustomerId;
    use std::sync::{Arc, Mutex};
    use user_core::role::UserRole;
    use user_core::tier::UserTier;
    use user_service::ports::UserDetailsView;

    #[derive(Default)]
    struct State {
        user: Option<UserDetailsView>,
        created_customers: usize,
        associated_customers: usize,
        checkout_sessions: usize,
        association_principal: Option<Principal>,
    }

    #[derive(Clone, Default)]
    struct Fakes(Arc<Mutex<State>>);

    impl Fakes {
        fn state(&self) -> std::sync::MutexGuard<'_, State> {
            match self.0.lock() {
                Ok(state) => state,
                Err(poisoned) => poisoned.into_inner(),
            }
        }
    }

    #[async_trait::async_trait]
    impl GetOwnUserUseCase for Fakes {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: GetOwnUserRequest,
        ) -> Result<UserDetailsView, GetOwnUserError> {
            self.state().user.clone().ok_or(GetOwnUserError::NotFound)
        }
    }

    #[async_trait::async_trait]
    impl AssociateUserStripeCustomerIdUseCase for Fakes {
        async fn execute(
            &self,
            context: &OperationContext,
            command: AssociateUserStripeCustomerIdCommand,
        ) -> Result<
            user_service::use_cases::AssociateUserStripeCustomerIdResult,
            AssociateUserStripeCustomerIdError,
        > {
            let mut state = self.state();
            state.associated_customers += 1;
            state.association_principal = Some(context.principal.clone());
            let mut view = state
                .user
                .clone()
                .ok_or(AssociateUserStripeCustomerIdError::UserNotFound)?;
            view.stripe_customer_id = Some(command.stripe_customer_id);
            Ok(user_service::use_cases::AssociateUserStripeCustomerIdResult { view })
        }
    }

    #[async_trait::async_trait]
    impl StripeCustomerCreator for Fakes {
        async fn create_customer(
            &self,
            _request: CreateStripeCustomerRequest,
        ) -> Result<StripeCustomerId, StripeBillingError> {
            self.state().created_customers += 1;
            Ok(StripeCustomerId::from("cus_created"))
        }
    }

    #[async_trait::async_trait]
    impl StripeCheckoutSessionCreator for Fakes {
        async fn create_checkout_session(
            &self,
            request: CreateStripeCheckoutSessionRequest,
        ) -> Result<Url, StripeBillingError> {
            let mut state = self.state();
            state.checkout_sessions += 1;
            assert_eq!("cus_created", request.stripe_customer_id.as_ref());
            assert_eq!("price_pro_monthly", request.price_id);
            Url::parse("https://checkout.stripe.test/session").map_err(|error| {
                StripeBillingError::InvalidResponse {
                    source: Box::new(error),
                }
            })
        }
    }

    fn handler(fakes: Fakes) -> CreateBillingCheckoutSessionHandler<Fakes, Fakes, Fakes, Fakes> {
        CreateBillingCheckoutSessionHandler::new(
            fakes.clone(),
            fakes.clone(),
            fakes.clone(),
            fakes,
            BillingPriceIds {
                pro_monthly: "price_pro_monthly".to_owned(),
                pro_yearly: "price_pro_yearly".to_owned(),
                ultimate_monthly: "price_ultimate_monthly".to_owned(),
                ultimate_yearly: "price_ultimate_yearly".to_owned(),
            },
        )
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: common::operation_context::RequestId::new("request"),
            correlation_id: common::operation_context::CorrelationId::new("correlation"),
        }
    }

    fn user(stripe_customer_id: Option<StripeCustomerId>) -> UserDetailsView {
        UserDetailsView {
            user_id: common::user_id::UserId::new(),
            email: serde_email::Email::try_from("ada@example.test")
                .unwrap_or_else(|error| panic!("invalid test email: {error}")),
            first_name: Some("Ada".into()),
            last_name: Some("Lovelace".into()),
            language: None,
            currency: None,
            measurement_unit: None,
            prohibited_content_consent: false,
            tier: UserTier::Free,
            role: UserRole::User,
            stripe_customer_id,
            structured_address: None,
            geo_address: None,
        }
    }

    #[tokio::test]
    async fn should_create_associate_then_create_checkout_session_for_authorized_user() {
        let fakes = Fakes::default();
        let user = user(None);
        let user_id = user.user_id;
        fakes.state().user = Some(user);

        let result = handler(fakes.clone())
            .execute(
                &context(Principal::User(user_id)),
                CreateBillingCheckoutSessionCommand {
                    plan: BillingPlan::Pro,
                    cycle: BillingCycle::Monthly,
                    idempotency_key: Some("request-key".to_owned()),
                },
            )
            .await;

        assert!(result.is_ok());
        let state = fakes.state();
        assert_eq!(1, state.created_customers);
        assert_eq!(1, state.associated_customers);
        assert_eq!(1, state.checkout_sessions);
        assert_eq!(
            Some(Principal::Service("billing-service".to_owned())),
            state.association_principal
        );
    }

    #[tokio::test]
    async fn should_reject_existing_customer_before_any_stripe_operation() {
        let fakes = Fakes::default();
        let user = user(Some(StripeCustomerId::from("cus_existing")));
        let user_id = user.user_id;
        fakes.state().user = Some(user);

        let result = handler(fakes.clone())
            .execute(
                &context(Principal::User(user_id)),
                CreateBillingCheckoutSessionCommand {
                    plan: BillingPlan::Pro,
                    cycle: BillingCycle::Monthly,
                    idempotency_key: None,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(CreateBillingCheckoutSessionError::StripeCustomerAlreadyExists)
        ));
        let state = fakes.state();
        assert_eq!(0, state.created_customers);
        assert_eq!(0, state.associated_customers);
        assert_eq!(0, state.checkout_sessions);
    }

    #[tokio::test]
    async fn should_reject_delegated_user_without_users_read_before_any_operation() {
        let fakes = Fakes::default();
        let result = handler(fakes.clone())
            .execute(
                &context(Principal::DelegatedUser {
                    user_id: common::user_id::UserId::new(),
                    capabilities: Default::default(),
                }),
                CreateBillingCheckoutSessionCommand {
                    plan: BillingPlan::Pro,
                    cycle: BillingCycle::Monthly,
                    idempotency_key: None,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(CreateBillingCheckoutSessionError::Forbidden)
        ));
        let state = fakes.state();
        assert_eq!(0, state.created_customers);
        assert_eq!(0, state.associated_customers);
        assert_eq!(0, state.checkout_sessions);
    }
}
