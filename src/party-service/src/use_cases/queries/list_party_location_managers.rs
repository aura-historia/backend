use crate::ports::party_location::PartyLocationAccessFactory;
use crate::ports::party_location_manager_reader::{
    PartyLocationManagerReader, PartyLocationManagerReaderFactory,
};
use crate::use_cases::party_locations::{PartyLocationError, authorize, privileged};
use application::{
    operation_context::OperationContext,
    pagination::Cursor,
    transaction::{Transaction, UnitOfWork},
};
use partnership_core::partnership_id::PartnershipId;
use party_core::party_id::PartyId;
use serde_email::Email;
use user_core::{first_name::FirstName, last_name::LastName, user_id::UserId};
use user_service::use_cases::queries::check_user_admin::CheckUserAdminUseCase;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartyLocationManagerAccessKind {
    Explicit,
    Partnership,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PartyLocationManagerAccess {
    Explicit,
    Partnership { partnership_ids: Vec<PartnershipId> },
}

/// Identity and independent current access reasons, without private profile data.
#[derive(Debug, Clone, PartialEq)]
pub struct PartyLocationManagerView {
    pub user_id: UserId,
    pub email: Email,
    pub first_name: Option<FirstName>,
    pub last_name: Option<LastName>,
    pub access: Vec<PartyLocationManagerAccess>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListPartyLocationManagersRequest {
    pub party_id: PartyId,
    pub user_id: Option<UserId>,
    pub access: Option<PartyLocationManagerAccessKind>,
    pub cursor: Cursor<UserId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartyLocationManagersPage {
    pub items: Vec<PartyLocationManagerView>,
    pub cursor: Cursor<UserId>,
}

#[async_trait::async_trait]
pub trait ListPartyLocationManagersUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListPartyLocationManagersRequest,
    ) -> Result<PartyLocationManagersPage, PartyLocationError>;
}

pub struct ListPartyLocationManagersHandler<U, R, F, A> {
    uow: U,
    reader: R,
    access: F,
    admin: A,
}

impl<U, R, F, A> ListPartyLocationManagersHandler<U, R, F, A> {
    pub fn new(uow: U, reader: R, access: F, admin: A) -> Self {
        Self {
            uow,
            reader,
            access,
            admin,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, F, A> ListPartyLocationManagersUseCase for ListPartyLocationManagersHandler<U, R, F, A>
where
    U: UnitOfWork,
    R: PartyLocationManagerReaderFactory<U::Tx>,
    F: PartyLocationAccessFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(name = "list_party_location_managers", skip_all, fields(
        principal_type = context.principal.kind(),
        actor_id = %context.principal.label(),
        request_id = %context.request_id,
        correlation_id = %context.correlation_id,
        party_id = %request.party_id,
    ))]
    async fn execute(
        &self,
        context: &OperationContext,
        mut request: ListPartyLocationManagersRequest,
    ) -> Result<PartyLocationManagersPage, PartyLocationError> {
        if !privileged(context, &self.admin, false).await? {
            return Err(PartyLocationError::Forbidden);
        }
        let mut tx = self.uow.begin().await?;
        authorize(context, true, &self.access, &mut tx, request.party_id).await?;
        request.cursor.size = request.cursor.size.clamp(1, 100);
        let result = self.reader.in_transaction(&mut tx).list(&request).await?;
        tx.commit().await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::party_location::PartyLocationAccess;
    use application::{
        operation_context::{CorrelationId, CredentialCapability, Principal, RequestId},
        transaction::TransactionError,
    };
    use user_service::use_cases::queries::check_user_admin::{
        CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminResult,
    };

    struct UnreachableStorage;
    struct Tx;
    #[async_trait::async_trait]
    impl Transaction for Tx {
        async fn commit(self) -> Result<(), TransactionError> {
            panic!("unauthorized commit");
        }
    }
    #[async_trait::async_trait]
    impl UnitOfWork for UnreachableStorage {
        type Tx = Tx;
        async fn begin(&self) -> Result<Tx, TransactionError> {
            panic!("unauthorized transaction");
        }
    }
    impl PartyLocationManagerReaderFactory<Tx> for UnreachableStorage {
        fn in_transaction<'tx>(&'tx self, _: &'tx mut Tx) -> impl PartyLocationManagerReader + 'tx {
            UnreachableStorage
        }
    }
    #[async_trait::async_trait]
    impl PartyLocationManagerReader for UnreachableStorage {
        async fn list(
            &mut self,
            _: &ListPartyLocationManagersRequest,
        ) -> Result<PartyLocationManagersPage, PartyLocationError> {
            panic!("unauthorized read");
        }
    }
    impl PartyLocationAccessFactory<Tx> for UnreachableStorage {
        fn in_transaction<'tx>(&'tx self, _: &'tx mut Tx) -> impl PartyLocationAccess + 'tx {
            UnreachableStorage
        }
    }
    #[async_trait::async_trait]
    impl PartyLocationAccess for UnreachableStorage {
        async fn lock_party(&mut self, _: PartyId) -> Result<bool, PartyLocationError> {
            panic!("unauthorized owner lock");
        }
        async fn can_administer_locations(
            &mut self,
            _: UserId,
        ) -> Result<bool, PartyLocationError> {
            panic!("unauthorized admin read");
        }
        async fn can_manage_locations(
            &mut self,
            _: PartyId,
            _: UserId,
        ) -> Result<bool, PartyLocationError> {
            panic!("directory must require an administrator");
        }
        async fn set_management_grant(
            &mut self,
            _: PartyId,
            _: UserId,
            _: bool,
            _: &str,
        ) -> Result<(), PartyLocationError> {
            panic!("directory must not write a grant");
        }
    }
    struct DeniedAdmin;
    #[async_trait::async_trait]
    impl CheckUserAdminUseCase for DeniedAdmin {
        async fn execute(
            &self,
            _: &OperationContext,
            _: CheckUserAdminRequest,
        ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
            Err(CheckUserAdminError::Forbidden)
        }
    }

    #[tokio::test]
    async fn rejects_missing_identity_scope_or_admin_role_before_accessing_party_storage() {
        let handler = ListPartyLocationManagersHandler::new(
            UnreachableStorage,
            UnreachableStorage,
            UnreachableStorage,
            DeniedAdmin,
        );
        let user_id = UserId::new();
        for principal in [
            Principal::Anonymous,
            Principal::User(user_id),
            Principal::DelegatedUser {
                user_id,
                capabilities: Default::default(),
            },
            Principal::DelegatedUser {
                user_id,
                capabilities: [CredentialCapability::PartiesWrite].into(),
            },
            Principal::DelegatedUser {
                user_id,
                capabilities: [CredentialCapability::PartiesRead].into(),
            },
        ] {
            let anonymous = principal == Principal::Anonymous;
            let context = OperationContext {
                principal,
                request_id: RequestId::new("test"),
                correlation_id: CorrelationId::new("test"),
            };
            let result = handler
                .execute(
                    &context,
                    ListPartyLocationManagersRequest {
                        party_id: PartyId::new(),
                        user_id: None,
                        access: None,
                        cursor: Cursor::default(),
                    },
                )
                .await;
            if anonymous {
                assert!(matches!(
                    result,
                    Err(PartyLocationError::AuthenticationRequired)
                ));
            } else {
                assert!(matches!(result, Err(PartyLocationError::Forbidden)));
            }
        }
    }
}
