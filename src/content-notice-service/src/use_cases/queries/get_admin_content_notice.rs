use crate::{
    admin_authorization::{AdminAuthorizationError, authorize_admin},
    ports::{
        ContentNoticeDetailsReadError, ContentNoticeDetailsReader,
        ContentNoticeDetailsReaderFactory, ContentNoticeStorageVersion,
    },
};
use application::{
    error::BoxError,
    operation_context::{CredentialCapability, OperationContext},
    transaction::{Transaction, UnitOfWork},
};
use content_notice_core::{content_notice::ContentNotice, content_notice_id::ContentNoticeId};
use listing_source_core::{ListingIngestionMethod, ListingSourceName, ListingSourceSlugId};
use product_listing_core::listing_lifecycle::ListingLifecycle;
use time::OffsetDateTime;
use user_service::ports::UserAdminReaderFactory;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GetAdminContentNoticeRequest {
    pub content_notice_id: ContentNoticeId,
}

/// One notice as an admin reviews it: the stored notice with its snapshot, and its target as
/// it is now.
#[derive(Debug, Clone, PartialEq)]
pub struct AdminContentNoticeDetails {
    pub notice: ContentNotice,
    pub version: ContentNoticeStorageVersion,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
    pub current_target: CurrentContentNoticeTarget,
}

/// The target as it is when the notice is read. Unlike the snapshot it follows every later
/// change, and a target that no longer exists is `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentContentNoticeTarget {
    pub listing_source: Option<CurrentListingSource>,
    /// Always `None` for a notice about a listing source.
    pub product_listing: Option<CurrentProductListing>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentListingSource {
    pub name: ListingSourceName,
    pub slug_id: ListingSourceSlugId,
    pub ingestion_methods: Vec<ListingIngestionMethod>,
    /// Whether the operator holds an active partnership now, under the same rule as the public
    /// listing source details.
    pub partner: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentProductListing {
    pub lifecycle: ListingLifecycle,
}

#[derive(Debug, thiserror::Error)]
pub enum GetAdminContentNoticeError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("content notice not found")]
    NotFound,
    #[error("temporary content notice read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid content notice read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal content notice read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin get admin content notice transaction")]
    BeginTransactionFailed(#[source] BoxError),
    #[error("failed to commit get admin content notice transaction")]
    CommitTransactionFailed(#[source] BoxError),
}

#[async_trait::async_trait]
pub trait GetAdminContentNoticeUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetAdminContentNoticeRequest,
    ) -> Result<AdminContentNoticeDetails, GetAdminContentNoticeError>;
}

pub struct GetAdminContentNoticeHandler<U, R, A> {
    unit_of_work: U,
    reader: R,
    admins: A,
}

impl<U, R, A> GetAdminContentNoticeHandler<U, R, A> {
    pub fn new(unit_of_work: U, reader: R, admins: A) -> Self {
        Self {
            unit_of_work,
            reader,
            admins,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> GetAdminContentNoticeUseCase for GetAdminContentNoticeHandler<U, R, A>
where
    U: UnitOfWork,
    R: ContentNoticeDetailsReaderFactory<U::Tx>,
    A: UserAdminReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "get_admin_content_notice",
        skip_all,
        fields(
            content_notice_id = %request.content_notice_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetAdminContentNoticeRequest,
    ) -> Result<AdminContentNoticeDetails, GetAdminContentNoticeError> {
        context
            .principal
            .require_credential_capability(CredentialCapability::ContentNoticesRead)
            .map_err(|_| GetAdminContentNoticeError::Forbidden)?;
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }

        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            GetAdminContentNoticeError::BeginTransactionFailed(Box::new(source))
        })?;
        authorize_admin(context, &mut tx, &self.admins).await?;
        let details = self
            .reader
            .in_transaction(&mut tx)
            .find_by_id(request.content_notice_id)
            .await?
            .ok_or(GetAdminContentNoticeError::NotFound)?;
        tx.commit().await.map_err(|source| {
            GetAdminContentNoticeError::CommitTransactionFailed(Box::new(source))
        })?;
        Ok(details)
    }
}

impl From<AdminAuthorizationError> for GetAdminContentNoticeError {
    fn from(error: AdminAuthorizationError) -> Self {
        match error {
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

impl From<ContentNoticeDetailsReadError> for GetAdminContentNoticeError {
    fn from(error: ContentNoticeDetailsReadError) -> Self {
        match error {
            ContentNoticeDetailsReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ContentNoticeDetailsReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::{
        error::static_error,
        operation_context::{CorrelationId, Principal, RequestId},
        transaction::TransactionError,
    };
    use content_notice_core::{
        additional_url::AdditionalUrls,
        content_notice::NewContentNotice,
        content_notice_target::{ContentNoticeTarget, ListingSourceSnapshot, TargetSnapshot},
        good_faith_statement_version::GoodFaithStatementVersion,
        reason_category::ReasonCategory,
        submitter::{Submitter, SubmitterEmail, SubmitterName},
        submitter_role::SubmitterRole,
    };
    use listing_source_core::ListingSourceId;
    use localization::Language;
    use std::{
        collections::BTreeSet,
        sync::{Arc, Mutex, MutexGuard},
    };
    use time::macros::datetime;
    use user_core::{role::UserRole, user_id::UserId};
    use user_service::ports::{UserAdminActorView, UserAdminReadError, UserAdminReader};

    #[derive(Default)]
    enum ReadOutcome {
        #[default]
        Found,
        Missing,
        Unavailable,
        Invalid,
    }

    #[derive(Default)]
    struct State {
        begins: usize,
        reads: usize,
        commits: usize,
        requested: Option<ContentNoticeId>,
        read_outcome: ReadOutcome,
        fail_begin: bool,
        fail_commit: bool,
    }

    type SharedState = Arc<Mutex<State>>;

    fn lock(state: &SharedState) -> MutexGuard<'_, State> {
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[derive(Clone)]
    struct FakeUnitOfWork(SharedState);

    struct FakeTransaction(SharedState);

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = lock(&self.0);
            if state.fail_commit {
                return Err(TransactionError::CommitFailed(static_error(
                    "commit failed",
                )));
            }
            state.commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let mut state = lock(&self.0);
            state.begins += 1;
            if state.fail_begin {
                return Err(TransactionError::BeginFailed(static_error("begin failed")));
            }
            Ok(FakeTransaction(Arc::clone(&self.0)))
        }
    }

    struct FakeReaderFactory(SharedState);

    struct FakeReader(SharedState);

    impl ContentNoticeDetailsReaderFactory<FakeTransaction> for FakeReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ContentNoticeDetailsReader + 'tx {
            FakeReader(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl ContentNoticeDetailsReader for FakeReader {
        async fn find_by_id(
            &mut self,
            content_notice_id: ContentNoticeId,
        ) -> Result<Option<AdminContentNoticeDetails>, ContentNoticeDetailsReadError> {
            let mut state = lock(&self.0);
            state.reads += 1;
            state.requested = Some(content_notice_id);
            match state.read_outcome {
                ReadOutcome::Found => Ok(Some(details(content_notice_id))),
                ReadOutcome::Missing => Ok(None),
                ReadOutcome::Unavailable => {
                    Err(ContentNoticeDetailsReadError::TemporarilyUnavailable {
                        source: static_error("connection lost"),
                    })
                }
                ReadOutcome::Invalid => Err(ContentNoticeDetailsReadError::InvalidReadModel {
                    source: static_error("invalid row"),
                }),
            }
        }
    }

    struct FakeAdminFactory(Option<UserRole>);

    struct FakeAdminReader(Option<UserRole>);

    impl UserAdminReaderFactory<FakeTransaction> for FakeAdminFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl UserAdminReader + 'tx {
            FakeAdminReader(self.0)
        }
    }

    #[async_trait::async_trait]
    impl UserAdminReader for FakeAdminReader {
        async fn find_admin_actor(
            &mut self,
            user_id: UserId,
        ) -> Result<Option<UserAdminActorView>, UserAdminReadError> {
            Ok(self.0.map(|role| UserAdminActorView { user_id, role }))
        }
    }

    fn details(id: ContentNoticeId) -> AdminContentNoticeDetails {
        let notice = ContentNotice::submit(NewContentNotice {
            id,
            target: ContentNoticeTarget::ListingSource {
                listing_source_id: ListingSourceId::new(),
            },
            target_snapshot: TargetSnapshot {
                listing_source: ListingSourceSnapshot {
                    name: ListingSourceName::try_from("Antik Shop").unwrap(),
                    url: None,
                    partner: false,
                },
                product_listing: None,
            },
            submitter_role: SubmitterRole::Provider,
            reason_category: ReasonCategory::ProviderOptOut,
            explanation: None,
            additional_urls: AdditionalUrls::default(),
            submitter: Submitter {
                name: SubmitterName::try_from("Erika Mustermann").unwrap(),
                email: SubmitterEmail::try_from("erika@antik.example").unwrap(),
                language: Language::De,
            },
            good_faith_statement_version: GoodFaithStatementVersion::GoodFaithV1,
        })
        .unwrap();
        AdminContentNoticeDetails {
            notice,
            version: ContentNoticeStorageVersion::default(),
            created: datetime!(2026-11-13 10:44 UTC),
            updated: datetime!(2026-11-13 10:44 UTC),
            current_target: CurrentContentNoticeTarget {
                listing_source: None,
                product_listing: None,
            },
        }
    }

    fn handler(
        state: &SharedState,
        role: Option<UserRole>,
    ) -> GetAdminContentNoticeHandler<FakeUnitOfWork, FakeReaderFactory, FakeAdminFactory> {
        GetAdminContentNoticeHandler::new(
            FakeUnitOfWork(Arc::clone(state)),
            FakeReaderFactory(Arc::clone(state)),
            FakeAdminFactory(role),
        )
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn delegated(capabilities: impl IntoIterator<Item = CredentialCapability>) -> Principal {
        Principal::DelegatedUser {
            user_id: UserId::new(),
            capabilities: capabilities.into_iter().collect::<BTreeSet<_>>(),
        }
    }

    async fn get_as_admin(
        state: &SharedState,
    ) -> Result<AdminContentNoticeDetails, GetAdminContentNoticeError> {
        handler(state, Some(UserRole::Admin))
            .execute(
                &context(Principal::User(UserId::new())),
                GetAdminContentNoticeRequest {
                    content_notice_id: ContentNoticeId::new(),
                },
            )
            .await
    }

    #[tokio::test]
    async fn should_authorize_admin_read_and_commit_in_one_transaction() {
        let state = SharedState::default();
        let content_notice_id = ContentNoticeId::new();

        let details = handler(&state, Some(UserRole::Admin))
            .execute(
                &context(delegated([CredentialCapability::ContentNoticesRead])),
                GetAdminContentNoticeRequest { content_notice_id },
            )
            .await
            .unwrap();

        assert_eq!(content_notice_id, details.notice.id());
        let state = lock(&state);
        assert_eq!((1, 1, 1), (state.begins, state.reads, state.commits));
        assert_eq!(Some(content_notice_id), state.requested);
    }

    #[tokio::test]
    async fn should_forbid_without_capability_admin_role_or_authentication() {
        let cases = [
            (Principal::Anonymous, Some(UserRole::Admin), 0),
            (delegated([]), Some(UserRole::Admin), 0),
            (
                delegated([CredentialCapability::AdminOverviewRead]),
                Some(UserRole::Admin),
                0,
            ),
            (
                delegated([CredentialCapability::ContentNoticesRead]),
                Some(UserRole::User),
                1,
            ),
            (Principal::User(UserId::new()), Some(UserRole::User), 1),
            (Principal::User(UserId::new()), None, 1),
        ];

        for (principal, role, expected_begins) in cases {
            let state = SharedState::default();

            let result = handler(&state, role)
                .execute(
                    &context(principal),
                    GetAdminContentNoticeRequest {
                        content_notice_id: ContentNoticeId::new(),
                    },
                )
                .await;

            assert!(matches!(result, Err(GetAdminContentNoticeError::Forbidden)));
            let state = lock(&state);
            assert_eq!(expected_begins, state.begins);
            assert_eq!((0, 0), (state.reads, state.commits));
        }
    }

    #[tokio::test]
    async fn should_report_unknown_notice_as_not_found_without_committing() {
        let state = SharedState::default();
        lock(&state).read_outcome = ReadOutcome::Missing;

        let result = get_as_admin(&state).await;

        assert!(matches!(result, Err(GetAdminContentNoticeError::NotFound)));
        assert_eq!(0, lock(&state).commits);
    }

    #[tokio::test]
    async fn should_keep_read_failure_classes() {
        let state = SharedState::default();
        lock(&state).read_outcome = ReadOutcome::Unavailable;
        assert!(matches!(
            get_as_admin(&state).await,
            Err(GetAdminContentNoticeError::TemporarilyUnavailable { .. })
        ));

        let state = SharedState::default();
        lock(&state).read_outcome = ReadOutcome::Invalid;
        assert!(matches!(
            get_as_admin(&state).await,
            Err(GetAdminContentNoticeError::InvalidReadModel { .. })
        ));
    }

    #[tokio::test]
    async fn should_report_begin_and_commit_failures_as_transaction_failures() {
        let state = SharedState::default();
        lock(&state).fail_begin = true;
        assert!(matches!(
            get_as_admin(&state).await,
            Err(GetAdminContentNoticeError::BeginTransactionFailed(_))
        ));
        assert_eq!(0, lock(&state).reads);

        let state = SharedState::default();
        lock(&state).fail_commit = true;
        assert!(matches!(
            get_as_admin(&state).await,
            Err(GetAdminContentNoticeError::CommitTransactionFailed(_))
        ));
        assert_eq!(1, lock(&state).reads);
    }
}
