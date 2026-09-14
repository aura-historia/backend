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
    Auction, AuctionDescription, AuctionFormat, AuctionId, AuctionKey, AuctionName,
    AuctionReportedStatus, AuctionSchedule, NewAuction, ReportedCatalogueLotCount, SourceAuctionId,
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
    pub description: Option<Localized<Language, AuctionDescription>>,
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
    #[tracing::instrument(name = "create_auction", skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
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

        let mut auction = Auction::create(NewAuction {
            id: AuctionId::new(),
            key: AuctionKey::new(command.listing_source_id, command.source_auction_id),
            name: command.name,
            description: command.description,
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

        tracing::info!(event = "auction.created", auction_id = %auction.id(), actor_type = context.principal.kind(), outcome = "success");
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
