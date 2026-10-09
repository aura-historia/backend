use crate::{
    ports::admin_auction_search_reader::{
        AdminAuctionSearchCursor, AdminAuctionSearchItem, AdminAuctionSearchReadError,
        AdminAuctionSearchReader, AdminAuctionSearchRequest,
    },
    use_cases::queries::get_auction::{AuctionAdminDetailsView, ensure_admin},
};
use application::{
    error::BoxError,
    operation_context::{CredentialAuthorizationError, CredentialCapability, OperationContext},
    pagination::CursoredResult,
};
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminUseCase,
};

pub type SearchAdminAuctionsResult =
    CursoredResult<AuctionAdminDetailsView, AdminAuctionSearchCursor>;

#[derive(Debug, thiserror::Error)]
pub enum SearchAdminAuctionsError {
    #[error("authenticated actor required to search auctions")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("auction search cursor belongs to another filter or sort scope")]
    CursorScopeMismatch,
    #[error("auction search page size must be between 1 and 100")]
    InvalidPageSize,
    #[error("temporary auction search failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted auction search state")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal auction search failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait SearchAdminAuctionsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: AdminAuctionSearchRequest,
    ) -> Result<SearchAdminAuctionsResult, SearchAdminAuctionsError>;
}

pub struct SearchAdminAuctionsHandler<R, A> {
    reader: R,
    check_user_admin: A,
}

impl<R, A> SearchAdminAuctionsHandler<R, A> {
    pub fn new(reader: R, check_user_admin: A) -> Self {
        Self {
            reader,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<R, A> SearchAdminAuctionsUseCase for SearchAdminAuctionsHandler<R, A>
where
    R: AdminAuctionSearchReader,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(name = "search_admin_auctions", skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: AdminAuctionSearchRequest,
    ) -> Result<SearchAdminAuctionsResult, SearchAdminAuctionsError> {
        context
            .require_credential_capability(CredentialCapability::AuctionsRead)
            .map_err(|error| match error {
                CredentialAuthorizationError::AuthenticationRequired(_) => {
                    SearchAdminAuctionsError::AuthenticatedActorRequired
                }
                CredentialAuthorizationError::InsufficientCapability { .. } => {
                    SearchAdminAuctionsError::Forbidden
                }
            })?;
        ensure_admin(
            context,
            &self.check_user_admin,
            map_admin_error,
            SearchAdminAuctionsError::AuthenticatedActorRequired,
        )
        .await?;
        validate_request(&request)?;
        Ok(self.reader.search(&request).await?.map_item(Into::into))
    }
}

fn validate_request(request: &AdminAuctionSearchRequest) -> Result<(), SearchAdminAuctionsError> {
    if !(1..=100).contains(&request.cursor.size) {
        return Err(SearchAdminAuctionsError::InvalidPageSize);
    }
    if request.cursor.search_after.as_ref().is_some_and(|after| {
        after.scope != request.scope
            || (request.scope.sort
                != crate::ports::admin_auction_search_reader::AdminAuctionSort::Name
                && after.sort_name.is_some())
    }) {
        return Err(SearchAdminAuctionsError::CursorScopeMismatch);
    }
    Ok(())
}

impl From<AdminAuctionSearchItem> for AuctionAdminDetailsView {
    fn from(item: AdminAuctionSearchItem) -> Self {
        Self {
            auction_id: item.auction_id,
            key: item.key,
            name: item.name,
            catalogue_url: item.catalogue_url,
            format: item.format,
            schedule: item.schedule,
            reported_status: item.reported_status,
            reported_lot_count: item.reported_lot_count,
            version: item.version,
            created: item.created,
            updated: item.updated,
        }
    }
}

fn map_admin_error(error: CheckUserAdminError) -> SearchAdminAuctionsError {
    match error {
        CheckUserAdminError::AuthenticatedActorRequired => {
            SearchAdminAuctionsError::AuthenticatedActorRequired
        }
        CheckUserAdminError::Forbidden => SearchAdminAuctionsError::Forbidden,
        CheckUserAdminError::TemporarilyUnavailable { source }
        | CheckUserAdminError::BeginTransactionFailed(source)
        | CheckUserAdminError::CommitTransactionFailed(source) => {
            SearchAdminAuctionsError::TemporarilyUnavailable { source }
        }
        CheckUserAdminError::InvalidReadModel { source }
        | CheckUserAdminError::Internal { source } => SearchAdminAuctionsError::Internal { source },
    }
}

impl From<AdminAuctionSearchReadError> for SearchAdminAuctionsError {
    fn from(error: AdminAuctionSearchReadError) -> Self {
        match error {
            AdminAuctionSearchReadError::QueryFailed { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AdminAuctionSearchReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::admin_auction_search_reader::{AdminAuctionSearchScope, AdminAuctionSort};
    use application::operation_context::{CorrelationId, Principal, RequestId};
    use application::pagination::Cursor;
    use auction_core::AuctionId;
    use domain_primitives::sort::SortOrder;
    use std::collections::BTreeSet;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use time::OffsetDateTime;
    use user_core::user_id::UserId;
    use user_service::use_cases::queries::check_user_admin::{
        CheckUserAdminRequest, CheckUserAdminResult,
    };

    struct CountingReader(Arc<AtomicUsize>);

    #[async_trait::async_trait]
    impl AdminAuctionSearchReader for CountingReader {
        async fn search(
            &self,
            _request: &AdminAuctionSearchRequest,
        ) -> Result<
            crate::ports::admin_auction_search_reader::AdminAuctionSearchReadResult,
            AdminAuctionSearchReadError,
        > {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(CursoredResult::default())
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

    #[tokio::test]
    async fn allows_delegated_admin_with_auctions_read_scope() {
        let reads = Arc::new(AtomicUsize::new(0));
        let admin_checks = Arc::new(AtomicUsize::new(0));
        let handler = SearchAdminAuctionsHandler::new(
            CountingReader(reads.clone()),
            AdminCheckFake {
                calls: admin_checks.clone(),
                is_admin: true,
            },
        );
        let result = handler
            .execute(
                &context(Principal::DelegatedUser {
                    user_id: UserId::new(),
                    capabilities: BTreeSet::from([CredentialCapability::AuctionsRead]),
                }),
                request(),
            )
            .await
            .expect("scoped admin can search auctions");

        assert_eq!(CursoredResult::default(), result);
        assert_eq!(1, admin_checks.load(Ordering::SeqCst));
        assert_eq!(1, reads.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn denies_delegated_users_without_auctions_read_before_admin_check_or_reading() {
        let reads = Arc::new(AtomicUsize::new(0));
        let admin_checks = Arc::new(AtomicUsize::new(0));
        let handler = SearchAdminAuctionsHandler::new(
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
                        request(),
                    )
                    .await,
                Err(SearchAdminAuctionsError::Forbidden)
            ));
            assert_eq!(0, admin_checks.load(Ordering::SeqCst));
            assert_eq!(0, reads.load(Ordering::SeqCst));
        }
    }

    #[tokio::test]
    async fn denies_non_admin_users_before_reading() {
        let reads = Arc::new(AtomicUsize::new(0));
        let admin_checks = Arc::new(AtomicUsize::new(0));
        let handler = SearchAdminAuctionsHandler::new(
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
                handler.execute(&context(principal), request()).await,
                Err(SearchAdminAuctionsError::Forbidden)
            ));
        }
        assert_eq!(2, admin_checks.load(Ordering::SeqCst));
        assert_eq!(0, reads.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn denies_anonymous_before_admin_check_or_reading() {
        let reads = Arc::new(AtomicUsize::new(0));
        let admin_checks = Arc::new(AtomicUsize::new(0));
        let handler = SearchAdminAuctionsHandler::new(
            CountingReader(reads.clone()),
            AdminCheckFake {
                calls: admin_checks.clone(),
                is_admin: true,
            },
        );

        assert!(matches!(
            handler
                .execute(&context(Principal::Anonymous), request())
                .await,
            Err(SearchAdminAuctionsError::AuthenticatedActorRequired)
        ));
        assert_eq!(0, admin_checks.load(Ordering::SeqCst));
        assert_eq!(0, reads.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn allows_first_party_admin_without_delegated_scopes() {
        let reads = Arc::new(AtomicUsize::new(0));
        let admin_checks = Arc::new(AtomicUsize::new(0));
        let handler = SearchAdminAuctionsHandler::new(
            CountingReader(reads.clone()),
            AdminCheckFake {
                calls: admin_checks.clone(),
                is_admin: true,
            },
        );
        let result = handler
            .execute(&context(Principal::User(UserId::new())), request())
            .await
            .expect("first-party admin can search auctions");

        assert_eq!(CursoredResult::default(), result);
        assert_eq!(1, admin_checks.load(Ordering::SeqCst));
        assert_eq!(1, reads.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn allows_service_and_system_without_admin_check() {
        let reads = Arc::new(AtomicUsize::new(0));
        let admin_checks = Arc::new(AtomicUsize::new(0));
        let handler = SearchAdminAuctionsHandler::new(
            CountingReader(reads.clone()),
            AdminCheckFake {
                calls: admin_checks.clone(),
                is_admin: false,
            },
        );

        for principal in [Principal::Service("test-service".into()), Principal::System] {
            let result = handler
                .execute(&context(principal), request())
                .await
                .expect("trusted principal can search auctions");
            assert_eq!(CursoredResult::default(), result);
        }
        assert_eq!(0, admin_checks.load(Ordering::SeqCst));
        assert_eq!(2, reads.load(Ordering::SeqCst));
    }

    fn request() -> AdminAuctionSearchRequest {
        AdminAuctionSearchRequest {
            scope: AdminAuctionSearchScope {
                query: None,
                listing_source_id: None,
                source_auction_id: None,
                format: None,
                reported_status: None,
                sort: AdminAuctionSort::Updated,
                order: SortOrder::Desc,
            },
            cursor: Cursor::default(),
        }
    }

    #[test]
    fn rejects_invalid_page_size_and_cross_scope_cursors() {
        let mut value = request();
        value.cursor.size = 0;
        assert!(matches!(
            validate_request(&value),
            Err(SearchAdminAuctionsError::InvalidPageSize)
        ));
        value.cursor.size = 101;
        assert!(matches!(
            validate_request(&value),
            Err(SearchAdminAuctionsError::InvalidPageSize)
        ));
        value.cursor.size = 21;
        let mut different = value.scope.clone();
        different.query = Some("other".into());
        value.cursor.search_after = Some(AdminAuctionSearchCursor {
            auction_id: AuctionId::new(),
            sort_name: None,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
            scope: different,
        });
        assert!(matches!(
            validate_request(&value),
            Err(SearchAdminAuctionsError::CursorScopeMismatch)
        ));
        value.cursor.search_after.as_mut().expect("cursor").scope = value.scope.clone();
        assert!(validate_request(&value).is_ok());
        for (sort, order) in [
            (AdminAuctionSort::Name, SortOrder::Desc),
            (AdminAuctionSort::Updated, SortOrder::Asc),
            (AdminAuctionSort::Created, SortOrder::Desc),
        ] {
            let mut other = value.clone();
            other.scope.sort = sort;
            other.scope.order = order;
            assert!(matches!(
                validate_request(&other),
                Err(SearchAdminAuctionsError::CursorScopeMismatch)
            ));
        }
        let mut other = value.clone();
        other.scope.listing_source_id = Some(listing_source_core::ListingSourceId::new());
        assert!(matches!(
            validate_request(&other),
            Err(SearchAdminAuctionsError::CursorScopeMismatch)
        ));
        value
            .cursor
            .search_after
            .as_mut()
            .expect("cursor")
            .sort_name = Some("wrong".into());
        assert!(matches!(
            validate_request(&value),
            Err(SearchAdminAuctionsError::CursorScopeMismatch)
        ));
    }
}
