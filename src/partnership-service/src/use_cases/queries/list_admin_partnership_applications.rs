use crate::{
    admin_authorization::{AdminAuthorizationError, authorize_admin},
    ports::{PartnershipApplicationReader, PartnershipApplicationReaderFactory},
};
use application::{
    error::BoxError,
    operation_context::OperationContext,
    pagination::{Cursor, CursoredResult},
    transaction::{Transaction, UnitOfWork},
};
use domain_primitives::sort::Sort;
use listing_source_core::ListingSourceId;
use partnership_core::{
    partnership_application::PartnershipProposal,
    partnership_application_id::PartnershipApplicationId,
    partnership_application_search::PartnershipApplicationSearch,
    partnership_application_state::PartnershipApplicationState, partnership_id::PartnershipId,
    sort_partnership_application_field::SortPartnershipApplicationField,
};
use time::OffsetDateTime;
use user_core::user_id::UserId;
use user_service::ports::UserAdminReaderFactory;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartnershipApplicationSearchCursor {
    pub position: OffsetDateTime,
    pub application_id: PartnershipApplicationId,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ListAdminPartnershipApplicationsRequest {
    pub search: PartnershipApplicationSearch,
    pub sort: Option<Sort<SortPartnershipApplicationField>>,
    pub cursor: Option<Cursor<PartnershipApplicationSearchCursor>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdminPartnershipApplicationSummary {
    pub id: PartnershipApplicationId,
    pub applicant_user_id: UserId,
    pub state: PartnershipApplicationState,
    pub proposal: PartnershipProposal,
    pub approved_partnership_id: Option<PartnershipId>,
    pub approved_listing_source_id: Option<ListingSourceId>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

pub type ListAdminPartnershipApplicationsResult =
    CursoredResult<AdminPartnershipApplicationSummary, PartnershipApplicationSearchCursor>;

#[derive(Debug, thiserror::Error)]
pub enum ListAdminPartnershipApplicationsError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin transaction")]
    BeginTransactionFailed,
    #[error("failed to commit transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait ListAdminPartnershipApplicationsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListAdminPartnershipApplicationsRequest,
    ) -> Result<ListAdminPartnershipApplicationsResult, ListAdminPartnershipApplicationsError>;
}

pub struct ListAdminPartnershipApplicationsHandler<U, A, R> {
    unit_of_work: U,
    reader: A,
    admins: R,
}

impl<U, A, R> ListAdminPartnershipApplicationsHandler<U, A, R> {
    pub fn new(unit_of_work: U, reader: A, admins: R) -> Self {
        Self {
            unit_of_work,
            reader,
            admins,
        }
    }
}

#[async_trait::async_trait]
impl<U: UnitOfWork, A: PartnershipApplicationReaderFactory<U::Tx>, R: UserAdminReaderFactory<U::Tx>>
    ListAdminPartnershipApplicationsUseCase for ListAdminPartnershipApplicationsHandler<U, A, R>
{
    #[tracing::instrument(
        name = "list_admin_partnership_applications",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListAdminPartnershipApplicationsRequest,
    ) -> Result<ListAdminPartnershipApplicationsResult, ListAdminPartnershipApplicationsError> {
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ListAdminPartnershipApplicationsError::BeginTransactionFailed)?;
        authorize_admin(context, &mut tx, &self.admins).await?;
        let result = self
            .reader
            .in_transaction(&mut tx)
            .search_admin(&request)
            .await?;
        tracing::Span::current().record("result_count", result.items.len());
        tx.commit()
            .await
            .map_err(|_| ListAdminPartnershipApplicationsError::CommitTransactionFailed)?;
        Ok(result)
    }
}

impl From<AdminAuthorizationError> for ListAdminPartnershipApplicationsError {
    fn from(v: AdminAuthorizationError) -> Self {
        match v {
            AdminAuthorizationError::Forbidden => Self::Forbidden,
            AdminAuthorizationError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AdminAuthorizationError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
            AdminAuthorizationError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<crate::ports::PartnershipApplicationReadError> for ListAdminPartnershipApplicationsError {
    fn from(v: crate::ports::PartnershipApplicationReadError) -> Self {
        match v {
            crate::ports::PartnershipApplicationReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            crate::ports::PartnershipApplicationReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
            crate::ports::PartnershipApplicationReadError::Internal { source } => {
                Self::Internal { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        PartnershipApplicationReadError, PartnershipApplicationReader,
        PartnershipApplicationReaderFactory,
    };
    use application::{
        operation_context::{CorrelationId, Principal, RequestId},
        transaction::TransactionError,
    };
    use std::sync::{Arc, Mutex};
    use user_core::role::UserRole;
    use user_service::ports::{
        UserAdminActorView, UserAdminReadError, UserAdminReader, UserAdminReaderFactory,
    };

    #[derive(Default)]
    struct State {
        begins: usize,
        searches: usize,
        commits: usize,
        request: Option<ListAdminPartnershipApplicationsRequest>,
    }

    #[derive(Clone)]
    struct FakeUnitOfWork(Arc<Mutex<State>>);

    struct FakeTransaction(Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = self
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let mut state = self
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.begins += 1;
            drop(state);
            Ok(FakeTransaction(Arc::clone(&self.0)))
        }
    }

    #[derive(Clone)]
    struct FakeReaderFactory {
        state: Arc<Mutex<State>>,
        fails: bool,
    }

    struct FakeReader {
        state: Arc<Mutex<State>>,
        fails: bool,
    }

    impl PartnershipApplicationReaderFactory<FakeTransaction> for FakeReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl PartnershipApplicationReader + 'tx {
            FakeReader {
                state: Arc::clone(&self.state),
                fails: self.fails,
            }
        }
    }

    #[async_trait::async_trait]
    impl PartnershipApplicationReader for FakeReader {
        async fn list_by_user(
            &mut self,
            _user_id: UserId,
        ) -> Result<Vec<crate::ports::PartnershipApplicationView>, PartnershipApplicationReadError>
        {
            Ok(Vec::new())
        }

        async fn search_admin(
            &mut self,
            request: &ListAdminPartnershipApplicationsRequest,
        ) -> Result<ListAdminPartnershipApplicationsResult, PartnershipApplicationReadError>
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.searches += 1;
            state.request = Some(request.clone());
            drop(state);
            if self.fails {
                return Err(PartnershipApplicationReadError::Internal {
                    source: application::error::static_error("reader failed"),
                });
            }
            Ok(CursoredResult::default())
        }
    }

    #[derive(Clone)]
    struct FakeAdminFactory {
        actor: Option<UserAdminActorView>,
    }

    struct FakeAdminReader {
        actor: Option<UserAdminActorView>,
    }

    impl UserAdminReaderFactory<FakeTransaction> for FakeAdminFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl UserAdminReader + 'tx {
            FakeAdminReader {
                actor: self.actor.clone(),
            }
        }
    }

    #[async_trait::async_trait]
    impl UserAdminReader for FakeAdminReader {
        async fn find_admin_actor(
            &mut self,
            _user_id: UserId,
        ) -> Result<Option<UserAdminActorView>, UserAdminReadError> {
            Ok(self.actor.clone())
        }
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn request() -> ListAdminPartnershipApplicationsRequest {
        ListAdminPartnershipApplicationsRequest {
            search: PartnershipApplicationSearch::default(),
            sort: Some(Sort {
                sort: SortPartnershipApplicationField::Updated,
                order: domain_primitives::sort::SortOrder::Asc,
            }),
            cursor: Some(Cursor {
                size: 2,
                search_after: Some(PartnershipApplicationSearchCursor {
                    position: time::macros::datetime!(2026-09-04 12:00 UTC),
                    application_id: PartnershipApplicationId::new(),
                }),
            }),
        }
    }

    fn handler(
        state: Arc<Mutex<State>>,
        actor: Option<UserAdminActorView>,
        fails: bool,
    ) -> ListAdminPartnershipApplicationsHandler<FakeUnitOfWork, FakeReaderFactory, FakeAdminFactory>
    {
        ListAdminPartnershipApplicationsHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            FakeReaderFactory { state, fails },
            FakeAdminFactory { actor },
        )
    }

    #[tokio::test]
    async fn should_authorize_admin_forward_request_and_commit() {
        let state = Arc::new(Mutex::new(State::default()));
        let admin_id = UserId::new();
        let expected_request = request();
        let result = handler(
            Arc::clone(&state),
            Some(UserAdminActorView {
                user_id: admin_id,
                role: UserRole::Admin,
            }),
            false,
        )
        .execute(
            &context(Principal::User(admin_id)),
            expected_request.clone(),
        )
        .await;

        assert!(result.is_ok());
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(1, state.begins);
        assert_eq!(1, state.searches);
        assert_eq!(1, state.commits);
        assert_eq!(Some(expected_request), state.request);
    }

    #[tokio::test]
    async fn should_reject_non_admin_before_searching() {
        let state = Arc::new(Mutex::new(State::default()));
        let user_id = UserId::new();
        let result = handler(
            Arc::clone(&state),
            Some(UserAdminActorView {
                user_id,
                role: UserRole::User,
            }),
            false,
        )
        .execute(&context(Principal::User(user_id)), request())
        .await;

        assert!(matches!(
            result,
            Err(ListAdminPartnershipApplicationsError::Forbidden)
        ));
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(1, state.begins);
        assert_eq!(0, state.searches);
        assert_eq!(0, state.commits);
    }

    #[tokio::test]
    async fn should_reject_anonymous_before_searching() {
        let state = Arc::new(Mutex::new(State::default()));
        let result = handler(Arc::clone(&state), None, false)
            .execute(&context(Principal::Anonymous), request())
            .await;

        assert!(matches!(
            result,
            Err(ListAdminPartnershipApplicationsError::Forbidden)
        ));
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(1, state.begins);
        assert_eq!(0, state.searches);
        assert_eq!(0, state.commits);
    }

    #[tokio::test]
    async fn should_map_reader_failure_without_committing() {
        let state = Arc::new(Mutex::new(State::default()));
        let admin_id = UserId::new();
        let result = handler(
            Arc::clone(&state),
            Some(UserAdminActorView {
                user_id: admin_id,
                role: UserRole::Admin,
            }),
            true,
        )
        .execute(&context(Principal::User(admin_id)), request())
        .await;

        assert!(matches!(
            result,
            Err(ListAdminPartnershipApplicationsError::Internal { .. })
        ));
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(1, state.searches);
        assert_eq!(0, state.commits);
    }
}
