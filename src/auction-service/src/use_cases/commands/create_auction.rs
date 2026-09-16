use crate::{
    ports::{
        AuctionEventAppendError, AuctionEventAppender, AuctionEventAppenderFactory,
        AuctionRepository, AuctionRepositoryError, AuctionRepositoryFactory,
    },
    use_cases::queries::get_auction::AuctionAdminDetailsView,
};
use application::{
    error::{BoxError, static_error},
    operation_context::OperationContext,
    transaction::{Transaction, UnitOfWork},
};
use auction_core::{
    Auction, AuctionFormat, AuctionId, AuctionKey, AuctionName, AuctionReportedStatus,
    AuctionSchedule, NewAuction, ReportedCatalogueLotCount, SourceAuctionId,
};
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use time::OffsetDateTime;
use url::Url;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminUseCase,
};

#[derive(Debug, Clone, PartialEq)]
pub struct CreateAuctionCommand {
    pub listing_source_id: ListingSourceId,
    pub source_auction_id: SourceAuctionId,
    pub name: Option<Localized<Language, AuctionName>>,
    pub catalogue_url: Option<Url>,
    pub format: Option<AuctionFormat>,
    pub schedule: AuctionSchedule,
    pub reported_status: Option<AuctionReportedStatus>,
    pub reported_lot_count: Option<ReportedCatalogueLotCount>,
}

pub type CreateAuctionResult = AuctionAdminDetailsView;

#[derive(Debug, thiserror::Error)]
pub enum CreateAuctionError {
    #[error("authenticated actor required to create auction")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("listing source not found")]
    ListingSourceNotFound,
    #[error("source auction key already exists")]
    SourceAuctionAlreadyExists,
    #[error("temporary auction persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted auction state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("auction event persistence failed")]
    EventPersistenceFailed {
        #[source]
        source: BoxError,
    },
    #[error("internal auction failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin create auction transaction")]
    BeginTransactionFailed,
    #[error("failed to commit create auction transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait CreateAuctionUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateAuctionCommand,
    ) -> Result<CreateAuctionResult, CreateAuctionError>;
}

pub struct CreateAuctionHandler<U, R, E, A> {
    unit_of_work: U,
    auctions: R,
    events: E,
    check_user_admin: A,
}

impl<U, R, E, A> CreateAuctionHandler<U, R, E, A> {
    pub fn new(unit_of_work: U, auctions: R, events: E, check_user_admin: A) -> Self {
        Self {
            unit_of_work,
            auctions,
            events,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, E, A> CreateAuctionUseCase for CreateAuctionHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: AuctionRepositoryFactory<U::Tx>,
    E: AuctionEventAppenderFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(
        name = "create_auction",
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
        command: CreateAuctionCommand,
    ) -> Result<CreateAuctionResult, CreateAuctionError> {
        super::super::queries::get_auction::ensure_admin(
            context,
            &self.check_user_admin,
            map_admin_error,
            CreateAuctionError::AuthenticatedActorRequired,
        )
        .await?;

        let actor_id = context.principal.actor_id();
        if let Some(actor_id) = actor_id.as_deref() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }

        let mut auction = Auction::create(NewAuction {
            id: AuctionId::new(),
            key: AuctionKey::new(command.listing_source_id, command.source_auction_id),
            name: command.name,
            catalogue_url: command.catalogue_url,
            format: command.format,
            schedule: command.schedule,
            reported_status: command.reported_status,
            reported_lot_count: command.reported_lot_count,
        })
        .map_err(|error| CreateAuctionError::Internal {
            source: Box::new(error),
        })?;
        let event = crate::ports::stamp_auction_event(
            auction.id(),
            OffsetDateTime::now_utc(),
            auction
                .take_pending_event_payload()
                .ok_or_else(|| CreateAuctionError::Internal {
                    source: static_error("new auction has no discovery event"),
                })?,
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| CreateAuctionError::BeginTransactionFailed)?;
        let stored = self
            .auctions
            .in_transaction(&mut tx)
            .insert(&auction)
            .await?;
        self.events.in_transaction(&mut tx).append(&event).await?;
        tx.commit()
            .await
            .map_err(|_| CreateAuctionError::CommitTransactionFailed)?;

        tracing::info!(
            event = "auction.created",
            actor_type = context.principal.kind(),
            actor_id = %actor_id.as_deref().unwrap_or(""),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
            auction_id = %auction.id(),
            outcome = "success",
        );
        Ok(AuctionAdminDetailsView::from_details(
            crate::ports::AuctionDetails { stored },
        ))
    }
}

fn map_admin_error(error: CheckUserAdminError) -> CreateAuctionError {
    match error {
        CheckUserAdminError::AuthenticatedActorRequired => {
            CreateAuctionError::AuthenticatedActorRequired
        }
        CheckUserAdminError::Forbidden => CreateAuctionError::Forbidden,
        CheckUserAdminError::TemporarilyUnavailable { source } => {
            CreateAuctionError::TemporarilyUnavailable { source }
        }
        CheckUserAdminError::InvalidReadModel { source }
        | CheckUserAdminError::Internal { source } => CreateAuctionError::Internal { source },
        CheckUserAdminError::BeginTransactionFailed
        | CheckUserAdminError::CommitTransactionFailed => {
            CreateAuctionError::TemporarilyUnavailable {
                source: static_error("check user admin transaction failed"),
            }
        }
    }
}

impl From<AuctionRepositoryError> for CreateAuctionError {
    fn from(error: AuctionRepositoryError) -> Self {
        match error {
            AuctionRepositoryError::SourceAuctionAlreadyExists { .. } => {
                Self::SourceAuctionAlreadyExists
            }
            AuctionRepositoryError::ListingSourceNotFound { .. } => Self::ListingSourceNotFound,
            AuctionRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AuctionRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            AuctionRepositoryError::ConcurrencyConflict
            | AuctionRepositoryError::Internal { .. } => Self::Internal {
                source: Box::new(error),
            },
        }
    }
}

impl From<AuctionEventAppendError> for CreateAuctionError {
    fn from(error: AuctionEventAppendError) -> Self {
        Self::EventPersistenceFailed {
            source: Box::new(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        AuctionEvent, AuctionEventAppendError, AuctionEventAppender, AuctionEventAppenderFactory,
        AuctionRepository, AuctionRepositoryError, AuctionRepositoryFactory, AuctionStorageVersion,
        StoredAuction,
    };
    use application::{
        operation_context::{CorrelationId, Principal, RequestId},
        transaction::{Transaction, TransactionError, UnitOfWork},
    };
    use auction_core::SourceAuctionId;
    use listing_source_core::ListingSourceId;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex, MutexGuard};
    use time::OffsetDateTime;
    use user_core::user_id::UserId;
    use user_service::use_cases::queries::check_user_admin::{
        CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminResult, CheckUserAdminUseCase,
    };

    #[derive(Default)]
    struct State {
        begins: usize,
        commits: usize,
        rollbacks: usize,
        inserts: usize,
        event_appends: usize,
        admin_checks: usize,
        repository_transaction_ids: Vec<u64>,
        event_transaction_ids: Vec<u64>,
        insert_results: VecDeque<Result<(), AuctionRepositoryError>>,
        event_results: VecDeque<Result<(), AuctionEventAppendError>>,
        admin_results: VecDeque<Result<CheckUserAdminResult, CheckUserAdminError>>,
    }

    type SharedState = Arc<Mutex<State>>;

    #[derive(Clone)]
    struct UnitOfWorkFake(SharedState);

    struct TransactionFake {
        id: u64,
        state: SharedState,
        committed: bool,
    }

    impl Drop for TransactionFake {
        fn drop(&mut self) {
            if !self.committed {
                lock(&self.state).rollbacks += 1;
            }
        }
    }

    #[derive(Clone)]
    struct AuctionsFake(SharedState);

    struct AuctionRepositoryFake<'tx> {
        state: SharedState,
        tx: &'tx mut TransactionFake,
    }

    #[derive(Clone)]
    struct EventsFake(SharedState);

    struct EventAppenderFake<'tx> {
        state: SharedState,
        tx: &'tx mut TransactionFake,
    }

    #[derive(Clone)]
    struct AdminCheckFake(SharedState);

    fn lock(state: &SharedState) -> MutexGuard<'_, State> {
        match state.lock() {
            Ok(state) => state,
            Err(error) => error.into_inner(),
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for UnitOfWorkFake {
        type Tx = TransactionFake;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let id = {
                let mut state = lock(&self.0);
                state.begins += 1;
                state.begins as u64
            };
            Ok(TransactionFake {
                id,
                state: Arc::clone(&self.0),
                committed: false,
            })
        }
    }

    #[async_trait::async_trait]
    impl Transaction for TransactionFake {
        async fn commit(mut self) -> Result<(), TransactionError> {
            self.committed = true;
            lock(&self.state).commits += 1;
            Ok(())
        }
    }

    impl AuctionRepositoryFactory<TransactionFake> for AuctionsFake {
        fn in_transaction<'tx>(
            &'tx self,
            tx: &'tx mut TransactionFake,
        ) -> impl AuctionRepository + 'tx {
            AuctionRepositoryFake {
                state: Arc::clone(&self.0),
                tx,
            }
        }
    }

    #[async_trait::async_trait]
    impl AuctionRepository for AuctionRepositoryFake<'_> {
        async fn find_by_id(
            &mut self,
            _: AuctionId,
        ) -> Result<Option<StoredAuction>, AuctionRepositoryError> {
            Ok(None)
        }

        async fn insert(
            &mut self,
            auction: &Auction,
        ) -> Result<StoredAuction, AuctionRepositoryError> {
            let transaction_id = self.tx.id;
            let mut state = lock(&self.state);
            state.inserts += 1;
            state.repository_transaction_ids.push(transaction_id);
            match state.insert_results.pop_front().unwrap_or(Ok(())) {
                Ok(()) => {
                    let now = OffsetDateTime::now_utc();
                    Ok(StoredAuction {
                        auction: auction.clone(),
                        version: AuctionStorageVersion::INITIAL,
                        created: now,
                        updated: now,
                    })
                }
                Err(error) => Err(error),
            }
        }

        async fn update(
            &mut self,
            _: &Auction,
            _: AuctionStorageVersion,
        ) -> Result<StoredAuction, AuctionRepositoryError> {
            Err(AuctionRepositoryError::Internal {
                source: Box::new(std::io::Error::other("auction update not used")),
            })
        }
    }

    impl AuctionEventAppenderFactory<TransactionFake> for EventsFake {
        fn in_transaction<'tx>(
            &'tx self,
            tx: &'tx mut TransactionFake,
        ) -> impl AuctionEventAppender + 'tx {
            EventAppenderFake {
                state: Arc::clone(&self.0),
                tx,
            }
        }
    }

    #[async_trait::async_trait]
    impl AuctionEventAppender for EventAppenderFake<'_> {
        async fn append(&mut self, _: &AuctionEvent) -> Result<(), AuctionEventAppendError> {
            let transaction_id = self.tx.id;
            let mut state = lock(&self.state);
            state.event_appends += 1;
            state.event_transaction_ids.push(transaction_id);
            state.event_results.pop_front().unwrap_or(Ok(()))
        }
    }

    #[async_trait::async_trait]
    impl CheckUserAdminUseCase for AdminCheckFake {
        async fn execute(
            &self,
            _: &OperationContext,
            _: CheckUserAdminRequest,
        ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
            let mut state = lock(&self.0);
            state.admin_checks += 1;
            state
                .admin_results
                .pop_front()
                .unwrap_or(Ok(CheckUserAdminResult))
        }
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::User(UserId::new()),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn command() -> CreateAuctionCommand {
        CreateAuctionCommand {
            listing_source_id: ListingSourceId::new(),
            source_auction_id: SourceAuctionId::try_from("source-auction")
                .unwrap_or_else(|error| panic!("valid source auction ID: {error}")),
            name: None,
            catalogue_url: None,
            format: None,
            schedule: AuctionSchedule::default(),
            reported_status: None,
            reported_lot_count: None,
        }
    }

    fn handler(
        state: &SharedState,
    ) -> CreateAuctionHandler<UnitOfWorkFake, AuctionsFake, EventsFake, AdminCheckFake> {
        CreateAuctionHandler::new(
            UnitOfWorkFake(Arc::clone(state)),
            AuctionsFake(Arc::clone(state)),
            EventsFake(Arc::clone(state)),
            AdminCheckFake(Arc::clone(state)),
        )
    }

    fn repository_failure() -> AuctionRepositoryError {
        AuctionRepositoryError::Internal {
            source: Box::new(std::io::Error::other("auction insert failed")),
        }
    }

    fn event_append_failure() -> AuctionEventAppendError {
        AuctionEventAppendError::AuctionEventAppendFailed {
            source: Box::new(std::io::Error::other("auction event append failed")),
        }
    }

    #[tokio::test]
    async fn should_create_once_in_one_transaction_and_commit_once() {
        let state = Arc::new(Mutex::new(State::default()));

        let result = handler(&state).execute(&context(), command()).await;

        assert!(result.is_ok());
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 1, 0));
        assert_eq!(
            (state.inserts, state.event_appends, state.admin_checks),
            (1, 1, 1)
        );
        assert_eq!(state.repository_transaction_ids, vec![1]);
        assert_eq!(state.event_transaction_ids, vec![1]);
    }

    #[tokio::test]
    async fn should_reject_anonymous_before_beginning_mutation_transaction() {
        let state = Arc::new(Mutex::new(State::default()));
        let mut context = context();
        context.principal = Principal::Anonymous;

        let result = handler(&state).execute(&context, command()).await;

        assert!(matches!(
            result,
            Err(CreateAuctionError::AuthenticatedActorRequired)
        ));
        let state = lock(&state);
        assert_eq!(
            (
                state.begins,
                state.commits,
                state.rollbacks,
                state.inserts,
                state.event_appends,
                state.admin_checks,
            ),
            (0, 0, 0, 0, 0, 0)
        );
    }

    #[tokio::test]
    async fn should_reject_forbidden_before_beginning_mutation_transaction() {
        let state = Arc::new(Mutex::new(State {
            admin_results: VecDeque::from([Err(CheckUserAdminError::Forbidden)]),
            ..Default::default()
        }));

        let result = handler(&state).execute(&context(), command()).await;

        assert!(matches!(result, Err(CreateAuctionError::Forbidden)));
        let state = lock(&state);
        assert_eq!(
            (
                state.begins,
                state.commits,
                state.rollbacks,
                state.inserts,
                state.event_appends,
                state.admin_checks,
            ),
            (0, 0, 0, 0, 0, 1)
        );
    }

    #[tokio::test]
    async fn should_not_commit_when_repository_insert_fails() {
        let state = Arc::new(Mutex::new(State {
            insert_results: VecDeque::from([Err(repository_failure())]),
            ..Default::default()
        }));

        let result = handler(&state).execute(&context(), command()).await;

        assert!(matches!(result, Err(CreateAuctionError::Internal { .. })));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 0, 1));
        assert_eq!((state.inserts, state.event_appends), (1, 0));
        assert_eq!(state.repository_transaction_ids, vec![1]);
        assert!(state.event_transaction_ids.is_empty());
    }

    #[tokio::test]
    async fn should_not_commit_when_event_append_fails() {
        let state = Arc::new(Mutex::new(State {
            event_results: VecDeque::from([Err(event_append_failure())]),
            ..Default::default()
        }));

        let result = handler(&state).execute(&context(), command()).await;

        assert!(matches!(
            result,
            Err(CreateAuctionError::EventPersistenceFailed { .. })
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 0, 1));
        assert_eq!((state.inserts, state.event_appends), (1, 1));
        assert_eq!(state.repository_transaction_ids, vec![1]);
        assert_eq!(state.event_transaction_ids, vec![1]);
    }
}
