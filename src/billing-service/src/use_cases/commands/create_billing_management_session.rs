use super::create_billing_checkout_session::{
    BillingCycle, BillingPlan, BillingPriceIds, BillingSessionResult, billing_service_context,
    user_name,
};
use crate::ports::{
    CreateStripeCheckoutSessionRequest, CreateStripeCustomerRequest,
    CreateStripePortalSessionRequest, StripeBillingError, StripeCheckoutSessionCreator,
    StripeCustomerCreator, StripePortalSessionCreator,
};
use application::error::BoxError;
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use user_core::tier::UserTier;
use user_service::use_cases::{
    AssociateUserStripeCustomerIdCommand, AssociateUserStripeCustomerIdError,
    AssociateUserStripeCustomerIdUseCase, GetOwnUserError, GetOwnUserRequest, GetOwnUserUseCase,
};

#[derive(Debug, Clone, PartialEq)]
pub struct CreateBillingManagementSessionCommand {
    pub plan: BillingPlan,
    pub cycle: BillingCycle,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateBillingManagementSessionError {
    #[error("authenticated actor required to create a billing management session")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("user not found")]
    UserNotFound,
    #[error("Stripe customer does not exist")]
    StripeCustomerDoesNotExist,
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
    #[error("billing internal failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait CreateBillingManagementSessionUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateBillingManagementSessionCommand,
    ) -> Result<BillingSessionResult, CreateBillingManagementSessionError>;
}

pub struct CreateBillingManagementSessionHandler<U, A, C, S, P> {
    users: U,
    associate_customer: A,
    customers: C,
    checkout_sessions: S,
    portal_sessions: P,
    prices: BillingPriceIds,
}

impl<U, A, C, S, P> CreateBillingManagementSessionHandler<U, A, C, S, P> {
    pub fn new(
        users: U,
        associate_customer: A,
        customers: C,
        checkout_sessions: S,
        portal_sessions: P,
        prices: BillingPriceIds,
    ) -> Self {
        Self {
            users,
            associate_customer,
            customers,
            checkout_sessions,
            portal_sessions,
            prices,
        }
    }
}

#[async_trait::async_trait]
impl<U, A, C, S, P> CreateBillingManagementSessionUseCase
    for CreateBillingManagementSessionHandler<U, A, C, S, P>
where
    U: GetOwnUserUseCase,
    A: AssociateUserStripeCustomerIdUseCase,
    C: StripeCustomerCreator,
    S: StripeCheckoutSessionCreator,
    P: StripePortalSessionCreator,
{
    #[tracing::instrument(
        name = "create_billing_management_session",
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
        command: CreateBillingManagementSessionCommand,
    ) -> Result<BillingSessionResult, CreateBillingManagementSessionError> {
        let user_id = authorize_billing_user(context)?;
        tracing::Span::current().record("actor_id", tracing::field::display(user_id));
        let user = self.users.execute(context, GetOwnUserRequest).await?;
        let url = match user.tier {
            UserTier::Free => {
                let stripe_customer_id = match user.stripe_customer_id {
                    Some(stripe_customer_id) => stripe_customer_id,
                    None => {
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
                        stripe_customer_id
                    }
                };
                self.checkout_sessions
                    .create_checkout_session(CreateStripeCheckoutSessionRequest {
                        user_id,
                        stripe_customer_id,
                        price_id: self.prices.price_id(command.plan, command.cycle).to_owned(),
                        idempotency_key: command.idempotency_key,
                    })
                    .await?
            }
            UserTier::Pro | UserTier::Ultimate => {
                let stripe_customer_id = user
                    .stripe_customer_id
                    .ok_or(CreateBillingManagementSessionError::StripeCustomerDoesNotExist)?;
                self.portal_sessions
                    .create_portal_session(CreateStripePortalSessionRequest {
                        stripe_customer_id,
                        idempotency_key: command.idempotency_key,
                    })
                    .await?
            }
        };
        tracing::Span::current().record("outcome", "success");
        Ok(BillingSessionResult { url })
    }
}

fn authorize_billing_user(
    context: &OperationContext,
) -> Result<user_core::user_id::UserId, CreateBillingManagementSessionError> {
    context
        .require()
        .credential_capability(CredentialCapability::UsersRead)
        .any_user()
        .authorize::<CreateBillingManagementSessionError>()?;
    match context.principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Ok(user_id),
        Principal::Anonymous => {
            Err(CreateBillingManagementSessionError::AuthenticatedActorRequired)
        }
        Principal::Service(_) | Principal::System => {
            Err(CreateBillingManagementSessionError::Forbidden)
        }
    }
}

impl From<OperationAuthorizationError> for CreateBillingManagementSessionError {
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
impl From<GetOwnUserError> for CreateBillingManagementSessionError {
    fn from(error: GetOwnUserError) -> Self {
        match error {
            GetOwnUserError::AuthenticatedActorRequired => Self::AuthenticatedActorRequired,
            GetOwnUserError::Forbidden => Self::Forbidden,
            GetOwnUserError::NotFound => Self::UserNotFound,
            GetOwnUserError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            GetOwnUserError::BeginTransactionFailed | GetOwnUserError::CommitTransactionFailed => {
                Self::TemporarilyUnavailable {
                    source: Box::new(error),
                }
            }
            GetOwnUserError::InvalidReadModel { source } | GetOwnUserError::Internal { source } => {
                Self::Internal { source }
            }
        }
    }
}
impl From<AssociateUserStripeCustomerIdError> for CreateBillingManagementSessionError {
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
impl From<StripeBillingError> for CreateBillingManagementSessionError {
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
