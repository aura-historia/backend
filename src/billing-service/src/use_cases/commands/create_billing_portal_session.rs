use crate::ports::{
    CreateStripePortalSessionRequest, StripeBillingError, StripePortalSessionCreator,
};
use common::error::boxed::BoxError;
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use url::Url;
use user_service::use_cases::{GetOwnUserError, GetOwnUserRequest, GetOwnUserUseCase};

#[derive(Debug, Clone, PartialEq)]
pub struct CreateBillingPortalSessionCommand {
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateBillingPortalSessionResult {
    pub url: Url,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateBillingPortalSessionError {
    #[error("authenticated actor required to create a billing portal session")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("user not found")]
    UserNotFound,
    #[error("Stripe customer does not exist")]
    StripeCustomerDoesNotExist,
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
    #[error("user account read failed")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait CreateBillingPortalSessionUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateBillingPortalSessionCommand,
    ) -> Result<CreateBillingPortalSessionResult, CreateBillingPortalSessionError>;
}

pub struct CreateBillingPortalSessionHandler<U, S> {
    users: U,
    portal_sessions: S,
}

impl<U, S> CreateBillingPortalSessionHandler<U, S> {
    pub fn new(users: U, portal_sessions: S) -> Self {
        Self {
            users,
            portal_sessions,
        }
    }
}

#[async_trait::async_trait]
impl<U, S> CreateBillingPortalSessionUseCase for CreateBillingPortalSessionHandler<U, S>
where
    U: GetOwnUserUseCase,
    S: StripePortalSessionCreator,
{
    #[tracing::instrument(
        name = "create_billing_portal_session",
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
        command: CreateBillingPortalSessionCommand,
    ) -> Result<CreateBillingPortalSessionResult, CreateBillingPortalSessionError> {
        let user_id = authorize_billing_user(context)?;
        tracing::Span::current().record("actor_id", tracing::field::display(user_id));
        let user = self.users.execute(context, GetOwnUserRequest).await?;
        let stripe_customer_id = user
            .stripe_customer_id
            .ok_or(CreateBillingPortalSessionError::StripeCustomerDoesNotExist)?;
        let url = self
            .portal_sessions
            .create_portal_session(CreateStripePortalSessionRequest {
                stripe_customer_id,
                idempotency_key: command.idempotency_key,
            })
            .await?;
        tracing::Span::current().record("outcome", "success");
        Ok(CreateBillingPortalSessionResult { url })
    }
}

fn authorize_billing_user(
    context: &OperationContext,
) -> Result<common::user_id::UserId, CreateBillingPortalSessionError> {
    context
        .require()
        .credential_capability(CredentialCapability::UsersRead)
        .any_user()
        .authorize::<CreateBillingPortalSessionError>()?;
    match context.principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Ok(user_id),
        Principal::Anonymous => Err(CreateBillingPortalSessionError::AuthenticatedActorRequired),
        Principal::Service(_) | Principal::System => {
            Err(CreateBillingPortalSessionError::Forbidden)
        }
    }
}

impl From<OperationAuthorizationError> for CreateBillingPortalSessionError {
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

impl From<GetOwnUserError> for CreateBillingPortalSessionError {
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

impl From<StripeBillingError> for CreateBillingPortalSessionError {
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
