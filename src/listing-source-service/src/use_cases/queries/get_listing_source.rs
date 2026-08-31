use crate::ports::{ListingSourceDetails, ListingSourceDetailsReader, ListingSourceReadError};
use application::{
    error::{BoxError, static_error},
    operation_context::{OperationContext, Principal},
};
use listing_source_core::{ListingSourceId, ListingSourceSlugId};
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

#[derive(Debug, Clone, PartialEq)]
pub enum GetListingSourceRequest {
    ById(ListingSourceId),
    BySlug(ListingSourceSlugId),
}
pub type GetListingSourceResult = ListingSourceDetails;
#[derive(Debug, thiserror::Error)]
pub enum GetListingSourceError {
    #[error("authenticated actor required to get listing source")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("listing source not found")]
    NotFound,
    #[error("temporary listing source read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid listing source read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal listing source read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}
#[async_trait::async_trait]
pub trait GetListingSourceUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetListingSourceRequest,
    ) -> Result<GetListingSourceResult, GetListingSourceError>;
}
pub struct GetListingSourceHandler<R, A> {
    reader: R,
    check_user_admin: A,
}
impl<R, A> GetListingSourceHandler<R, A> {
    pub fn new(reader: R, check_user_admin: A) -> Self {
        Self {
            reader,
            check_user_admin,
        }
    }
}
#[async_trait::async_trait]
impl<R, A> GetListingSourceUseCase for GetListingSourceHandler<R, A>
where
    R: ListingSourceDetailsReader,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(name = "get_listing_source", skip_all, fields(principal_type = context.principal.kind(), actor_id = tracing::field::Empty, request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetListingSourceRequest,
    ) -> Result<GetListingSourceResult, GetListingSourceError> {
        ensure_admin(context, &self.check_user_admin).await?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );
        let value = match request {
            GetListingSourceRequest::ById(id) => self.reader.find_details_by_id(id).await,
            GetListingSourceRequest::BySlug(slug) => self.reader.find_details_by_slug(&slug).await,
        }
        .map_err(map_read)?;
        value.ok_or(GetListingSourceError::NotFound)
    }
}
async fn ensure_admin<A>(context: &OperationContext, check: &A) -> Result<(), GetListingSourceError>
where
    A: CheckUserAdminUseCase,
{
    match context.principal {
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::Anonymous => Err(GetListingSourceError::AuthenticatedActorRequired),
        Principal::User(_) | Principal::DelegatedUser { .. } => check
            .execute(context, CheckUserAdminRequest)
            .await
            .map(|_| ())
            .map_err(|error| match error {
                CheckUserAdminError::AuthenticatedActorRequired => {
                    GetListingSourceError::AuthenticatedActorRequired
                }
                CheckUserAdminError::Forbidden => GetListingSourceError::Forbidden,
                CheckUserAdminError::TemporarilyUnavailable { source } => {
                    GetListingSourceError::TemporarilyUnavailable { source }
                }
                CheckUserAdminError::InvalidReadModel { source }
                | CheckUserAdminError::Internal { source } => {
                    GetListingSourceError::Internal { source }
                }
                CheckUserAdminError::BeginTransactionFailed
                | CheckUserAdminError::CommitTransactionFailed => GetListingSourceError::Internal {
                    source: static_error("check user admin transaction failed"),
                },
            }),
    }
}
fn map_read(error: ListingSourceReadError) -> GetListingSourceError {
    match error {
        ListingSourceReadError::TemporarilyUnavailable { source } => {
            GetListingSourceError::TemporarilyUnavailable { source }
        }
        ListingSourceReadError::InvalidReadModel { source } => {
            GetListingSourceError::InvalidReadModel { source }
        }
    }
}
