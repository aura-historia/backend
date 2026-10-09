use crate::ports::{AuctionDetailsReadError, AuctionDetailsReader, AuctionStorageVersion};
use application::{
    error::BoxError,
    operation_context::{
        CredentialAuthorizationError, CredentialCapability, OperationContext, Principal,
    },
};
use auction_core::{
    AuctionFormat, AuctionId, AuctionKey, AuctionName, AuctionReportedStatus, AuctionSchedule,
    ReportedCatalogueLotCount,
};
use localization::{Language, Localized};

use time::OffsetDateTime;
use url::Url;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

#[derive(Debug, Clone, PartialEq)]
pub struct AuctionAdminDetailsView {
    pub auction_id: AuctionId,
    pub key: AuctionKey,
    pub name: Option<Localized<Language, AuctionName>>,
    pub catalogue_url: Option<Url>,
    pub format: Option<AuctionFormat>,
    pub schedule: AuctionSchedule,
    pub reported_status: Option<AuctionReportedStatus>,
    pub reported_lot_count: Option<ReportedCatalogueLotCount>,
    pub version: AuctionStorageVersion,

    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum GetAuctionError {
    #[error("authenticated actor required to get auction")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("auction not found")]
    NotFound,
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
    #[error("internal auction failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait GetAuctionUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        auction_id: AuctionId,
    ) -> Result<AuctionAdminDetailsView, GetAuctionError>;
}

pub struct GetAuctionHandler<R, A> {
    details: R,
    check_user_admin: A,
}

impl<R, A> GetAuctionHandler<R, A> {
    pub fn new(details: R, check_user_admin: A) -> Self {
        Self {
            details,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<R, A> GetAuctionUseCase for GetAuctionHandler<R, A>
where
    R: AuctionDetailsReader,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(name = "get_auction", skip_all, fields(auction_id = %auction_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        auction_id: AuctionId,
    ) -> Result<AuctionAdminDetailsView, GetAuctionError> {
        context
            .require_credential_capability(CredentialCapability::AuctionsRead)
            .map_err(|error| match error {
                CredentialAuthorizationError::AuthenticationRequired(_) => {
                    GetAuctionError::AuthenticatedActorRequired
                }
                CredentialAuthorizationError::InsufficientCapability { .. } => {
                    GetAuctionError::Forbidden
                }
            })?;
        ensure_admin(
            context,
            &self.check_user_admin,
            map_admin_error_for_get,
            GetAuctionError::AuthenticatedActorRequired,
        )
        .await?;
        let details = self
            .details
            .find_by_id(auction_id)
            .await?
            .ok_or(GetAuctionError::NotFound)?;
        Ok(AuctionAdminDetailsView::from_details(details))
    }
}

impl AuctionAdminDetailsView {
    pub fn from_details(details: crate::ports::AuctionDetails) -> Self {
        let stored = details.stored;
        let auction = stored.auction;
        Self {
            auction_id: auction.id(),
            key: auction.key().clone(),
            name: auction.name().cloned(),
            catalogue_url: auction.catalogue_url().cloned(),
            format: auction.format(),
            schedule: auction.schedule().clone(),
            reported_status: auction.reported_status(),
            reported_lot_count: auction.reported_lot_count(),
            version: stored.version,

            created: stored.created,
            updated: stored.updated,
        }
    }
}

pub(crate) async fn ensure_admin<A, E>(
    context: &OperationContext,
    check: &A,
    map: impl Fn(CheckUserAdminError) -> E,
    unauthenticated: E,
) -> Result<(), E>
where
    A: CheckUserAdminUseCase,
{
    match context.principal {
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::Anonymous => Err(unauthenticated),
        Principal::User(_) | Principal::DelegatedUser { .. } => check
            .execute(context, CheckUserAdminRequest)
            .await
            .map(|_| ())
            .map_err(map),
    }
}

pub(crate) fn map_admin_error_for_get(error: CheckUserAdminError) -> GetAuctionError {
    match error {
        CheckUserAdminError::AuthenticatedActorRequired => {
            GetAuctionError::AuthenticatedActorRequired
        }
        CheckUserAdminError::Forbidden => GetAuctionError::Forbidden,
        CheckUserAdminError::TemporarilyUnavailable { source } => {
            GetAuctionError::TemporarilyUnavailable { source }
        }
        CheckUserAdminError::InvalidReadModel { source }
        | CheckUserAdminError::Internal { source } => GetAuctionError::Internal { source },
        CheckUserAdminError::BeginTransactionFailed(source)
        | CheckUserAdminError::CommitTransactionFailed(source) => {
            GetAuctionError::TemporarilyUnavailable { source }
        }
    }
}

impl From<AuctionDetailsReadError> for GetAuctionError {
    fn from(error: AuctionDetailsReadError) -> Self {
        match error {
            AuctionDetailsReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AuctionDetailsReadError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            AuctionDetailsReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{AuctionDetails, StoredAuction};
    use application::operation_context::{CorrelationId, RequestId};
    use auction_core::{Auction, NewAuction, SourceAuctionId};
    use listing_source_core::ListingSourceId;
    use std::collections::BTreeSet;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use user_core::user_id::UserId;
    use user_service::use_cases::queries::check_user_admin::CheckUserAdminResult;

    struct CountingReader(Arc<AtomicUsize>);

    #[async_trait::async_trait]
    impl AuctionDetailsReader for CountingReader {
        async fn find_by_id(
            &self,
            id: AuctionId,
        ) -> Result<Option<AuctionDetails>, AuctionDetailsReadError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Some(auction_details(id)))
        }
    }

    struct AdminCheckFake {
        calls: Arc<AtomicUsize>,
        is_admin: bool,
    }

    #[async_trait::async_trait]
    impl CheckUserAdminUseCase for AdminCheckFake {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: CheckUserAdminRequest,
        ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.is_admin {
                Ok(CheckUserAdminResult)
            } else {
                Err(CheckUserAdminError::Forbidden)
            }
        }
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("test"),
            correlation_id: CorrelationId::new("test"),
        }
    }

    fn auction_details(id: AuctionId) -> AuctionDetails {
        let mut auction = Auction::create(NewAuction {
            id,
            key: AuctionKey::new(
                ListingSourceId::new(),
                SourceAuctionId::try_from("source-auction")
                    .unwrap_or_else(|error| panic!("valid source auction ID: {error}")),
            ),
            name: None,
            catalogue_url: None,
            format: None,
            schedule: AuctionSchedule::default(),
            reported_status: None,
            reported_lot_count: None,
        })
        .unwrap_or_else(|error| panic!("valid Auction: {error}"));
        let _ = auction.take_pending_event_payload();
        AuctionDetails {
            stored: StoredAuction {
                auction,
                version: AuctionStorageVersion::INITIAL,
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            },
        }
    }

    #[tokio::test]
    async fn allows_delegated_admin_with_auctions_read_scope() {
        let reads = Arc::new(AtomicUsize::new(0));
        let admin_checks = Arc::new(AtomicUsize::new(0));
        let handler = GetAuctionHandler::new(
            CountingReader(reads.clone()),
            AdminCheckFake {
                calls: admin_checks.clone(),
                is_admin: true,
            },
        );
        let auction_id = AuctionId::new();
        let result = handler
            .execute(
                &context(Principal::DelegatedUser {
                    user_id: UserId::new(),
                    capabilities: BTreeSet::from([CredentialCapability::AuctionsRead]),
                }),
                auction_id,
            )
            .await
            .expect("scoped admin can get auction");

        assert_eq!(auction_id, result.auction_id);
        assert_eq!(1, admin_checks.load(Ordering::SeqCst));
        assert_eq!(1, reads.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn denies_delegated_users_without_auctions_read_before_admin_check_or_reading() {
        let reads = Arc::new(AtomicUsize::new(0));
        let admin_checks = Arc::new(AtomicUsize::new(0));
        let handler = GetAuctionHandler::new(
            CountingReader(reads.clone()),
            AdminCheckFake {
                calls: admin_checks.clone(),
                is_admin: true,
            },
        );

        for capabilities in [
            BTreeSet::new(),
            BTreeSet::from([CredentialCapability::UsersRead]),
        ] {
            assert!(matches!(
                handler
                    .execute(
                        &context(Principal::DelegatedUser {
                            user_id: UserId::new(),
                            capabilities,
                        }),
                        AuctionId::new(),
                    )
                    .await,
                Err(GetAuctionError::Forbidden)
            ));
            assert_eq!(0, admin_checks.load(Ordering::SeqCst));
            assert_eq!(0, reads.load(Ordering::SeqCst));
        }
    }

    #[tokio::test]
    async fn denies_non_admin_users_before_reading() {
        let reads = Arc::new(AtomicUsize::new(0));
        let admin_checks = Arc::new(AtomicUsize::new(0));
        let handler = GetAuctionHandler::new(
            CountingReader(reads.clone()),
            AdminCheckFake {
                calls: admin_checks.clone(),
                is_admin: false,
            },
        );

        for principal in [
            Principal::DelegatedUser {
                user_id: UserId::new(),
                capabilities: BTreeSet::from([CredentialCapability::AuctionsRead]),
            },
            Principal::User(UserId::new()),
        ] {
            assert!(matches!(
                handler.execute(&context(principal), AuctionId::new()).await,
                Err(GetAuctionError::Forbidden)
            ));
        }
        assert_eq!(2, admin_checks.load(Ordering::SeqCst));
        assert_eq!(0, reads.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn denies_anonymous_before_admin_check_or_reading() {
        let reads = Arc::new(AtomicUsize::new(0));
        let admin_checks = Arc::new(AtomicUsize::new(0));
        let handler = GetAuctionHandler::new(
            CountingReader(reads.clone()),
            AdminCheckFake {
                calls: admin_checks.clone(),
                is_admin: true,
            },
        );

        assert!(matches!(
            handler
                .execute(&context(Principal::Anonymous), AuctionId::new())
                .await,
            Err(GetAuctionError::AuthenticatedActorRequired)
        ));
        assert_eq!(0, admin_checks.load(Ordering::SeqCst));
        assert_eq!(0, reads.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn allows_first_party_admin_without_delegated_scopes() {
        let reads = Arc::new(AtomicUsize::new(0));
        let admin_checks = Arc::new(AtomicUsize::new(0));
        let handler = GetAuctionHandler::new(
            CountingReader(reads.clone()),
            AdminCheckFake {
                calls: admin_checks.clone(),
                is_admin: true,
            },
        );
        let auction_id = AuctionId::new();
        let result = handler
            .execute(&context(Principal::User(UserId::new())), auction_id)
            .await
            .expect("first-party admin can get auction");

        assert_eq!(auction_id, result.auction_id);
        assert_eq!(1, admin_checks.load(Ordering::SeqCst));
        assert_eq!(1, reads.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn allows_service_and_system_without_admin_check() {
        let reads = Arc::new(AtomicUsize::new(0));
        let admin_checks = Arc::new(AtomicUsize::new(0));
        let handler = GetAuctionHandler::new(
            CountingReader(reads.clone()),
            AdminCheckFake {
                calls: admin_checks.clone(),
                is_admin: false,
            },
        );

        for principal in [Principal::Service("test-service".into()), Principal::System] {
            let auction_id = AuctionId::new();
            let result = handler
                .execute(&context(principal), auction_id)
                .await
                .expect("trusted principal can get auction");
            assert_eq!(auction_id, result.auction_id);
        }
        assert_eq!(0, admin_checks.load(Ordering::SeqCst));
        assert_eq!(2, reads.load(Ordering::SeqCst));
    }
}
