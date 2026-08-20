use crate::ports::{
    UserPartnerShopsReadError, UserPartnerShopsReader, UserPartnerShopsReaderFactory,
};
use application::error::BoxError;
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use application::transaction::{Transaction, UnitOfWork};
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use shop_core::shop_slug_id::ShopSlugId;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct ListPartnerShopsRequest {
    pub user_id: UserId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartnerShopSummary {
    pub shop_id: ShopId,
    pub shop_slug_id: ShopSlugId,
    pub name: ShopName,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListPartnerShopsResult {
    pub user_id: UserId,
    pub items: Vec<PartnerShopSummary>,
}

#[derive(Debug, thiserror::Error)]
pub enum ListPartnerShopsError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary user partner shops read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid user partner shops read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal user partner shops read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin list partner shops transaction")]
    BeginTransactionFailed,
    #[error("failed to commit list partner shops transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait ListPartnerShopsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListPartnerShopsRequest,
    ) -> Result<ListPartnerShopsResult, ListPartnerShopsError>;
}

pub struct ListPartnerShopsHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> ListPartnerShopsHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> ListPartnerShopsUseCase for ListPartnerShopsHandler<U, R>
where
    U: UnitOfWork,
    R: UserPartnerShopsReaderFactory<U::Tx>,
{
    #[tracing::instrument(name = "list_partner_shops", skip_all, fields(user_id = %request.user_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListPartnerShopsRequest,
    ) -> Result<ListPartnerShopsResult, ListPartnerShopsError> {
        authorize_list(context, request.user_id)?;
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ListPartnerShopsError::BeginTransactionFailed)?;
        let result = self
            .reader
            .in_transaction(&mut tx)
            .list_partner_shops(&request)
            .await?;
        tx.commit()
            .await
            .map_err(|_| ListPartnerShopsError::CommitTransactionFailed)?;
        Ok(result)
    }
}

fn authorize_list(
    context: &OperationContext,
    requested_user_id: UserId,
) -> Result<(), ListPartnerShopsError> {
    context
        .require()
        .credential_capability(CredentialCapability::PartnerShopsRead)
        .user(&requested_user_id)
        .service_or_system()
        .authorize::<ListPartnerShopsError>()
}

impl From<OperationAuthorizationError> for ListPartnerShopsError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => Self::Forbidden,
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}

impl From<UserPartnerShopsReadError> for ListPartnerShopsError {
    fn from(error: UserPartnerShopsReadError) -> Self {
        match error {
            UserPartnerShopsReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserPartnerShopsReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
            UserPartnerShopsReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::operation_context::{
        CorrelationId, CredentialCapability, Principal, RequestId,
    };
    use std::collections::BTreeSet;

    #[test]
    fn should_allow_user_to_list_own_partner_shops() {
        let user_id = UserId::new();

        let result = authorize_list(&context(Principal::User(user_id)), user_id);

        assert!(matches!(result, Ok(())));
    }

    #[test]
    fn should_allow_delegated_user_to_list_own_partner_shops() {
        let user_id = UserId::new();

        let result = authorize_list(
            &context(Principal::DelegatedUser {
                user_id,
                capabilities: BTreeSet::from([CredentialCapability::PartnerShopsRead]),
            }),
            user_id,
        );

        assert!(matches!(result, Ok(())));
    }

    #[test]
    fn should_reject_user_listing_other_user_partner_shops() {
        let result = authorize_list(&context(Principal::User(UserId::new())), UserId::new());

        assert!(matches!(result, Err(ListPartnerShopsError::Forbidden)));
    }

    #[test]
    fn should_reject_delegated_user_listing_other_user_partner_shops() {
        let result = authorize_list(
            &context(Principal::DelegatedUser {
                user_id: UserId::new(),
                capabilities: BTreeSet::new(),
            }),
            UserId::new(),
        );

        assert!(matches!(result, Err(ListPartnerShopsError::Forbidden)));
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::from("request"),
            correlation_id: CorrelationId::from("correlation"),
        }
    }
}
