use application::error::BoxError;
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use application::transaction::Transaction;
use user_core::role::UserRole;
use user_service::ports::{UserAdminReadError, UserAdminReader, UserAdminReaderFactory};

#[derive(Debug, thiserror::Error)]
pub(crate) enum AdminAuthorizationError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("operation authorization failed")]
    Operation(#[from] OperationAuthorizationError),
    #[error("temporary admin authorization failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid admin authorization read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal admin authorization failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

pub(crate) async fn authorize_admin_actor<Tx, R>(
    context: &OperationContext,
    tx: &mut Tx,
    reader: &R,
) -> Result<(), AdminAuthorizationError>
where
    Tx: Transaction,
    R: UserAdminReaderFactory<Tx>,
{
    context
        .require()
        .credential_capability(CredentialCapability::PartnerShopApplicationsWrite)
        .authorize::<OperationAuthorizationError>()?;

    match &context.principal {
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => {
            let user = reader
                .in_transaction(tx)
                .find_admin_actor(*user_id)
                .await?
                .ok_or(AdminAuthorizationError::Forbidden)?;
            if user.role == UserRole::Admin {
                Ok(())
            } else {
                Err(AdminAuthorizationError::Forbidden)
            }
        }
        Principal::Anonymous => Err(OperationAuthorizationError::AuthenticationRequired(
            application::operation_context::AuthenticationRequired,
        )
        .into()),
    }
}

impl From<UserAdminReadError> for AdminAuthorizationError {
    fn from(error: UserAdminReadError) -> Self {
        match error {
            UserAdminReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserAdminReadError::InvalidReadModel { source } => Self::InvalidReadModel { source },
            UserAdminReadError::Internal { source } => Self::Internal { source },
        }
    }
}
