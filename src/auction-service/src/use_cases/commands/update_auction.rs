use crate::{
    ports::{
        AuctionEventAppendError, AuctionEventAppender, AuctionEventAppenderFactory,
        AuctionRepository, AuctionRepositoryError, AuctionRepositoryFactory, AuctionStorageVersion,
    },
    use_cases::queries::get_auction::AuctionAdminDetailsView,
};
use application::{
    error::{BoxError, static_error},
    operation_context::OperationContext,
    patch_field::PatchField,
    transaction::{Transaction, UnitOfWork},
};
use auction_core::{
    AuctionDescription, AuctionFormat, AuctionId, AuctionName, AuctionReportedStatus,
    AuctionSchedule, ReplaceAuctionScheduleError, ReportedCatalogueLotCount,
};
use domain_primitives::change_outcome::ChangeOutcome;
use localization::{Language, Localized};
use time::OffsetDateTime;
use url::Url;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminUseCase,
};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AuctionSchedulePatch {
    pub bidding_opens: PatchField<OffsetDateTime>,
    pub live_starts: PatchField<OffsetDateTime>,
    pub lots_begin_closing: PatchField<OffsetDateTime>,
    pub scheduled_end: PatchField<OffsetDateTime>,
}

impl AuctionSchedulePatch {
    fn is_changed(&self) -> bool {
        self.bidding_opens.is_changed()
            || self.live_starts.is_changed()
            || self.lots_begin_closing.is_changed()
            || self.scheduled_end.is_changed()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateAuctionCommand {
    pub auction_id: AuctionId,
    pub expected_version: AuctionStorageVersion,
    pub name: PatchField<Localized<Language, AuctionName>>,
    pub description: PatchField<Localized<Language, AuctionDescription>>,
    pub catalogue_url: PatchField<Url>,
    pub format: PatchField<AuctionFormat>,
    pub schedule: AuctionSchedulePatch,
    pub reported_status: PatchField<AuctionReportedStatus>,
    pub reported_lot_count: PatchField<ReportedCatalogueLotCount>,
}

pub type UpdateAuctionResult = AuctionAdminDetailsView;

#[derive(Debug, thiserror::Error)]
pub enum UpdateAuctionError {
    #[error("authenticated actor required to update auction")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("auction not found")]
    NotFound,
    #[error("concurrent auction update")]
    ConcurrencyConflict,
    #[error("invalid auction schedule")]
    InvalidSchedule {
        #[source]
        source: ReplaceAuctionScheduleError,
    },
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
    #[error("failed to begin update auction transaction")]
    BeginTransactionFailed,
    #[error("failed to commit update auction transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait UpdateAuctionUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateAuctionCommand,
    ) -> Result<UpdateAuctionResult, UpdateAuctionError>;
}

pub struct UpdateAuctionHandler<U, R, E, A> {
    unit_of_work: U,
    auctions: R,
    events: E,
    check_user_admin: A,
}

impl<U, R, E, A> UpdateAuctionHandler<U, R, E, A> {
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
impl<U, R, E, A> UpdateAuctionUseCase for UpdateAuctionHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: AuctionRepositoryFactory<U::Tx>,
    E: AuctionEventAppenderFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(
        name = "update_auction",
        skip_all,
        fields(
            auction_id = %command.auction_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateAuctionCommand,
    ) -> Result<UpdateAuctionResult, UpdateAuctionError> {
        super::super::queries::get_auction::ensure_admin(
            context,
            &self.check_user_admin,
            map_admin_error,
            UpdateAuctionError::AuthenticatedActorRequired,
        )
        .await?;

        let actor_id = context.principal.actor_id();
        if let Some(actor_id) = actor_id.as_deref() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdateAuctionError::BeginTransactionFailed)?;
        let stored = self
            .auctions
            .in_transaction(&mut tx)
            .find_by_id(command.auction_id)
            .await?
            .ok_or(UpdateAuctionError::NotFound)?;
        if stored.version != command.expected_version {
            return Err(UpdateAuctionError::ConcurrencyConflict);
        }
        let mut auction = stored.auction.clone();
        let changed = apply_update(&mut auction, &command)?;
        let persisted = if changed.changed() {
            let event = crate::ports::stamp_auction_event(
                auction.id(),
                OffsetDateTime::now_utc(),
                auction.take_pending_event_payload().ok_or_else(|| {
                    UpdateAuctionError::Internal {
                        source: static_error("changed auction has no event"),
                    }
                })?,
            );
            let persisted = self
                .auctions
                .in_transaction(&mut tx)
                .update(&auction, stored.version)
                .await?;
            self.events.in_transaction(&mut tx).append(&event).await?;
            persisted
        } else {
            stored
        };
        tx.commit()
            .await
            .map_err(|_| UpdateAuctionError::CommitTransactionFailed)?;
        tracing::info!(
            event = "auction.updated",
            actor_type = context.principal.kind(),
            actor_id = %actor_id.as_deref().unwrap_or(""),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
            auction_id = %command.auction_id,
            changed = changed.changed(),
            outcome = "success",
        );
        Ok(AuctionAdminDetailsView::from_details(
            crate::ports::AuctionDetails { stored: persisted },
        ))
    }
}

fn apply_update(
    auction: &mut auction_core::Auction,
    command: &UpdateAuctionCommand,
) -> Result<ChangeOutcome, UpdateAuctionError> {
    let mut outcome = ChangeOutcome::Unchanged;
    outcome = outcome.combine(match &command.name {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Clear => auction.clear_name(),
        PatchField::Set(value) => auction.rename(value.clone()),
    });
    outcome = outcome.combine(match &command.description {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Clear => auction.clear_description(),
        PatchField::Set(value) => auction.replace_description(value.clone()),
    });
    outcome = outcome.combine(match &command.catalogue_url {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Clear => auction.clear_catalogue_url(),
        PatchField::Set(value) => auction.replace_catalogue_url(value.clone()),
    });
    outcome = outcome.combine(match &command.format {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Clear => auction.clear_format(),
        PatchField::Set(value) => auction.set_format(*value),
    });
    outcome = outcome.combine(match &command.reported_status {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Clear => auction.clear_reported_status(),
        PatchField::Set(value) => auction.set_reported_status(*value),
    });
    outcome = outcome.combine(match &command.reported_lot_count {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Clear => auction.clear_reported_lot_count(),
        PatchField::Set(value) => auction.set_reported_lot_count(*value),
    });
    if command.schedule.is_changed() {
        let current = auction.schedule();
        let schedule = AuctionSchedule::new(
            patch_option(current.bidding_opens(), &command.schedule.bidding_opens),
            patch_option(current.live_starts(), &command.schedule.live_starts),
            patch_option(
                current.lots_begin_closing(),
                &command.schedule.lots_begin_closing,
            ),
            patch_option(current.scheduled_end(), &command.schedule.scheduled_end),
        )
        .map_err(|source| UpdateAuctionError::InvalidSchedule {
            source: ReplaceAuctionScheduleError::InvalidSchedule(source),
        })?;
        outcome = outcome.combine(
            auction
                .replace_schedule(schedule)
                .map_err(|source| UpdateAuctionError::InvalidSchedule { source })?,
        );
    }
    Ok(outcome)
}

fn patch_option<T: Clone>(current: Option<T>, patch: &PatchField<T>) -> Option<T> {
    match patch {
        PatchField::Unchanged => current,
        PatchField::Set(value) => Some(value.clone()),
        PatchField::Clear => None,
    }
}

fn map_admin_error(error: CheckUserAdminError) -> UpdateAuctionError {
    match error {
        CheckUserAdminError::AuthenticatedActorRequired => {
            UpdateAuctionError::AuthenticatedActorRequired
        }
        CheckUserAdminError::Forbidden => UpdateAuctionError::Forbidden,
        CheckUserAdminError::TemporarilyUnavailable { source } => {
            UpdateAuctionError::TemporarilyUnavailable { source }
        }
        CheckUserAdminError::InvalidReadModel { source }
        | CheckUserAdminError::Internal { source } => UpdateAuctionError::Internal { source },
        CheckUserAdminError::BeginTransactionFailed
        | CheckUserAdminError::CommitTransactionFailed => {
            UpdateAuctionError::TemporarilyUnavailable {
                source: static_error("check user admin transaction failed"),
            }
        }
    }
}

impl From<AuctionRepositoryError> for UpdateAuctionError {
    fn from(error: AuctionRepositoryError) -> Self {
        match error {
            AuctionRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            AuctionRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AuctionRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            AuctionRepositoryError::SourceAuctionAlreadyExists { .. }
            | AuctionRepositoryError::ListingSourceNotFound { .. }
            | AuctionRepositoryError::Internal { .. } => Self::Internal {
                source: Box::new(error),
            },
        }
    }
}
impl From<AuctionEventAppendError> for UpdateAuctionError {
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
        AuctionRepository, AuctionRepositoryError, AuctionRepositoryFactory, StoredAuction,
    };
    use application::{
        operation_context::{CorrelationId, Principal, RequestId},
        transaction::{TransactionError, UnitOfWork},
    };
    use auction_core::{Auction, AuctionKey, NewAuction, SourceAuctionId};
    use listing_source_core::ListingSourceId;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex, MutexGuard};
    use time::macros::datetime;
    use user_core::user_id::UserId;
    use user_service::use_cases::queries::check_user_admin::{
        CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminResult, CheckUserAdminUseCase,
    };

    #[derive(Default)]
    struct State {
        begins: usize,
        commits: usize,
        rollbacks: usize,
        updates: usize,
        event_appends: usize,
        admin_checks: usize,
        repository_transaction_ids: Vec<u64>,
        event_transaction_ids: Vec<u64>,
        finds: VecDeque<Option<StoredAuction>>,
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
            Ok(lock(&self.state).finds.pop_front().flatten())
        }

        async fn insert(&mut self, _: &Auction) -> Result<StoredAuction, AuctionRepositoryError> {
            Err(AuctionRepositoryError::Internal {
                source: Box::new(std::io::Error::other("auction insert not used")),
            })
        }

        async fn update(
            &mut self,
            auction: &Auction,
            expected_version: AuctionStorageVersion,
        ) -> Result<StoredAuction, AuctionRepositoryError> {
            let transaction_id = self.tx.id;
            let mut state = lock(&self.state);
            state.updates += 1;
            state.repository_transaction_ids.push(transaction_id);
            let now = OffsetDateTime::now_utc();
            Ok(StoredAuction {
                auction: auction.clone(),
                version: expected_version.next(),
                created: now,
                updated: now,
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

    fn stored_auction() -> StoredAuction {
        let mut auction = Auction::create(NewAuction {
            id: AuctionId::new(),
            key: AuctionKey::new(
                ListingSourceId::new(),
                SourceAuctionId::try_from("source-auction")
                    .unwrap_or_else(|error| panic!("valid source auction ID: {error}")),
            ),
            name: None,
            description: None,
            catalogue_url: None,
            format: None,
            schedule: AuctionSchedule::default(),
            reported_status: None,
            reported_lot_count: None,
        })
        .unwrap_or_else(|error| panic!("valid Auction: {error}"));
        let _ = auction.take_pending_event_payload();
        let now = OffsetDateTime::now_utc();
        StoredAuction {
            auction,
            version: AuctionStorageVersion::INITIAL,
            created: now,
            updated: now,
        }
    }

    fn command(
        auction_id: AuctionId,
        expected_version: AuctionStorageVersion,
    ) -> UpdateAuctionCommand {
        UpdateAuctionCommand {
            auction_id,
            expected_version,
            name: PatchField::Unchanged,
            description: PatchField::Unchanged,
            catalogue_url: PatchField::Unchanged,
            format: PatchField::Unchanged,
            schedule: AuctionSchedulePatch::default(),
            reported_status: PatchField::Unchanged,
            reported_lot_count: PatchField::Unchanged,
        }
    }

    fn changed_command(
        auction_id: AuctionId,
        expected_version: AuctionStorageVersion,
    ) -> UpdateAuctionCommand {
        UpdateAuctionCommand {
            schedule: AuctionSchedulePatch {
                live_starts: PatchField::Set(datetime!(2026-10-18 16:03 UTC)),
                ..Default::default()
            },
            ..command(auction_id, expected_version)
        }
    }

    fn handler(
        state: &SharedState,
    ) -> UpdateAuctionHandler<UnitOfWorkFake, AuctionsFake, EventsFake, AdminCheckFake> {
        UpdateAuctionHandler::new(
            UnitOfWorkFake(Arc::clone(state)),
            AuctionsFake(Arc::clone(state)),
            EventsFake(Arc::clone(state)),
            AdminCheckFake(Arc::clone(state)),
        )
    }

    fn event_append_failure() -> AuctionEventAppendError {
        AuctionEventAppendError::AuctionEventAppendFailed {
            source: Box::new(std::io::Error::other("auction event append failed")),
        }
    }

    #[tokio::test]
    async fn should_return_concurrency_conflict_without_updating_or_appending_for_stale_version() {
        let stored = stored_auction();
        let auction_id = stored.auction.id();
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([Some(stored)]),
            ..Default::default()
        }));

        let result = handler(&state)
            .execute(
                &context(),
                command(auction_id, AuctionStorageVersion::INITIAL.next()),
            )
            .await;

        assert!(matches!(
            result,
            Err(UpdateAuctionError::ConcurrencyConflict)
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 0, 1));
        assert_eq!((state.updates, state.event_appends), (0, 0));
    }

    #[tokio::test]
    async fn should_commit_without_persistence_for_semantic_noop() {
        let stored = stored_auction();
        let auction_id = stored.auction.id();
        let expected_version = stored.version;
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([Some(stored)]),
            ..Default::default()
        }));

        let result = handler(&state)
            .execute(&context(), command(auction_id, expected_version))
            .await;

        assert!(matches!(
            result,
            Ok(AuctionAdminDetailsView { version, .. }) if version == expected_version
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 1, 0));
        assert_eq!((state.updates, state.event_appends), (0, 0));
    }

    #[tokio::test]
    async fn should_update_and_append_once_and_commit_once_for_real_change() {
        let stored = stored_auction();
        let auction_id = stored.auction.id();
        let expected_version = stored.version;
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([Some(stored)]),
            ..Default::default()
        }));

        let result = handler(&state)
            .execute(&context(), changed_command(auction_id, expected_version))
            .await;

        assert!(matches!(
            result,
            Ok(AuctionAdminDetailsView { version, .. }) if version == expected_version.next()
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 1, 0));
        assert_eq!((state.updates, state.event_appends), (1, 1));
        assert_eq!(state.repository_transaction_ids, vec![1]);
        assert_eq!(state.event_transaction_ids, vec![1]);
    }

    #[tokio::test]
    async fn should_not_commit_when_event_append_fails() {
        let stored = stored_auction();
        let auction_id = stored.auction.id();
        let expected_version = stored.version;
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([Some(stored)]),
            event_results: VecDeque::from([Err(event_append_failure())]),
            ..Default::default()
        }));

        let result = handler(&state)
            .execute(&context(), changed_command(auction_id, expected_version))
            .await;

        assert!(matches!(
            result,
            Err(UpdateAuctionError::EventPersistenceFailed { .. })
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 0, 1));
        assert_eq!((state.updates, state.event_appends), (1, 1));
    }

    #[tokio::test]
    async fn should_reject_authorization_before_beginning_mutation_transaction() {
        let state = Arc::new(Mutex::new(State {
            admin_results: VecDeque::from([Err(CheckUserAdminError::Forbidden)]),
            ..Default::default()
        }));

        let result = handler(&state)
            .execute(
                &context(),
                command(AuctionId::new(), AuctionStorageVersion::INITIAL),
            )
            .await;

        assert!(matches!(result, Err(UpdateAuctionError::Forbidden)));
        let state = lock(&state);
        assert_eq!(
            (
                state.begins,
                state.commits,
                state.rollbacks,
                state.updates,
                state.event_appends,
                state.admin_checks,
            ),
            (0, 0, 0, 0, 0, 1)
        );
    }

    #[test]
    fn should_replace_one_schedule_instant() {
        let mut auction = Auction::create(NewAuction {
            id: AuctionId::new(),
            key: AuctionKey::new(
                ListingSourceId::new(),
                SourceAuctionId::try_from("catalogue-42")
                    .unwrap_or_else(|error| panic!("valid source Auction ID: {error}")),
            ),
            name: None,
            description: None,
            catalogue_url: None,
            format: None,
            schedule: AuctionSchedule::default(),
            reported_status: None,
            reported_lot_count: None,
        })
        .unwrap_or_else(|error| panic!("valid Auction: {error}"));
        let command = UpdateAuctionCommand {
            auction_id: auction.id(),
            expected_version: AuctionStorageVersion::try_from(1_i64)
                .unwrap_or_else(|error| panic!("valid Auction version: {error}")),
            name: PatchField::Unchanged,
            description: PatchField::Unchanged,
            catalogue_url: PatchField::Unchanged,
            format: PatchField::Unchanged,
            schedule: AuctionSchedulePatch {
                live_starts: PatchField::Set(datetime!(2026-10-18 16:03 UTC)),
                ..Default::default()
            },
            reported_status: PatchField::Unchanged,
            reported_lot_count: PatchField::Unchanged,
        };

        let outcome = apply_update(&mut auction, &command)
            .unwrap_or_else(|error| panic!("valid schedule update: {error}"));

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert_eq!(
            Some(datetime!(2026-10-18 16:03 UTC)),
            auction.schedule().live_starts()
        );
    }
}
