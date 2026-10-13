use application::{
    error::BoxError,
    operation_context::{
        CredentialAuthorizationError, CredentialCapability, OperationContext, Principal,
    },
};
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

#[derive(Debug, thiserror::Error)]
pub enum AdminProductListingError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("administrator authority and credential capability required")]
    Forbidden,
    #[error("product listing not found")]
    NotFound,
    #[error("product listing version did not match")]
    ConcurrencyConflict,
    #[error("product listing is withdrawn")]
    ListingWithdrawn,
    #[error("invalid product listing patch")]
    InvalidPatch {
        #[source]
        source: BoxError,
    },
    #[error("auction not found")]
    AuctionNotFound,
    #[error("auction belongs to another listing source")]
    AuctionSourceMismatch,
    #[error("invalid page size")]
    InvalidPageSize,
    #[error("query must be NUL-free and at most 512 UTF-8 bytes")]
    InvalidQuery,
    #[error("cursor belongs to another filter or sort scope")]
    CursorScopeMismatch,
    #[error("temporary admin product listing dependency failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted product listing state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("admin product listing operation failed")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin admin product listing transaction")]
    BeginTransactionFailed(#[source] BoxError),
    #[error("failed to commit admin product listing transaction")]
    CommitTransactionFailed(#[source] BoxError),
}

pub(crate) async fn authorize_admin<A: CheckUserAdminUseCase>(
    context: &OperationContext,
    check: &A,
    capability: CredentialCapability,
) -> Result<(), AdminProductListingError> {
    context
        .require_credential_capability(capability)
        .map_err(|error| match error {
            CredentialAuthorizationError::AuthenticationRequired(_) => {
                AdminProductListingError::AuthenticatedActorRequired
            }
            CredentialAuthorizationError::InsufficientCapability { .. } => {
                AdminProductListingError::Forbidden
            }
        })?;
    match context.principal {
        Principal::Anonymous => Err(AdminProductListingError::AuthenticatedActorRequired),
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::User(_) | Principal::DelegatedUser { .. } => check
            .execute(context, CheckUserAdminRequest)
            .await
            .map(|_| ())
            .map_err(|error| match error {
                CheckUserAdminError::AuthenticatedActorRequired => {
                    AdminProductListingError::AuthenticatedActorRequired
                }
                CheckUserAdminError::Forbidden => AdminProductListingError::Forbidden,
                CheckUserAdminError::TemporarilyUnavailable { source }
                | CheckUserAdminError::BeginTransactionFailed(source)
                | CheckUserAdminError::CommitTransactionFailed(source) => {
                    AdminProductListingError::TemporarilyUnavailable { source }
                }
                CheckUserAdminError::InvalidReadModel { source }
                | CheckUserAdminError::Internal { source } => {
                    AdminProductListingError::Internal { source }
                }
            }),
    }
}
