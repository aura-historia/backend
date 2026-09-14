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
    AuctionSchedule, AuctionTime, ReplaceAuctionScheduleError, ReportedCatalogueLotCount,
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
    pub bidding_opens: PatchField<AuctionTime>,
    pub live_starts: PatchField<AuctionTime>,
    pub lots_begin_closing: PatchField<AuctionTime>,
    pub scheduled_end: PatchField<AuctionTime>,
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
    #[tracing::instrument(name = "update_auction", skip_all, fields(auction_id = %command.auction_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
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
        tracing::info!(event = "auction.updated", auction_id = %command.auction_id, actor_type = context.principal.kind(), changed = changed.changed(), outcome = "success");
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
            patch_option(
                current.bidding_opens().cloned(),
                &command.schedule.bidding_opens,
            ),
            patch_option(
                current.live_starts().cloned(),
                &command.schedule.live_starts,
            ),
            patch_option(
                current.lots_begin_closing().cloned(),
                &command.schedule.lots_begin_closing,
            ),
            patch_option(
                current.scheduled_end().cloned(),
                &command.schedule.scheduled_end,
            ),
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
