use crate::{
    ports::admin_product_listing_reader::*,
    use_cases::admin_product_listing_error::{AdminProductListingError, authorize_admin},
};
use application::operation_context::{CredentialCapability, OperationContext};
use product_listing_core::product_listing_id::ProductListingId;
use user_service::use_cases::queries::check_user_admin::CheckUserAdminUseCase;

#[async_trait::async_trait]
pub trait GetAdminProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        id: ProductListingId,
    ) -> Result<AdminProductListingDetails, AdminProductListingError>;
}

pub struct GetAdminProductListingHandler<R, A> {
    reader: R,
    check_user_admin: A,
}
impl<R, A> GetAdminProductListingHandler<R, A> {
    pub fn new(reader: R, check_user_admin: A) -> Self {
        Self {
            reader,
            check_user_admin,
        }
    }
}
#[async_trait::async_trait]
impl<R: AdminProductListingDetailsReader, A: CheckUserAdminUseCase> GetAdminProductListingUseCase
    for GetAdminProductListingHandler<R, A>
{
    #[tracing::instrument(name = "get_admin_product_listing", skip_all, fields(product_listing_id = %id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        id: ProductListingId,
    ) -> Result<AdminProductListingDetails, AdminProductListingError> {
        authorize_admin(
            context,
            &self.check_user_admin,
            CredentialCapability::ProductListingsRead,
        )
        .await?;
        self.reader
            .find_by_id(id)
            .await?
            .ok_or(AdminProductListingError::NotFound)
    }
}

#[async_trait::async_trait]
pub trait SearchAdminProductListingsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: AdminProductListingSearchRequest,
    ) -> Result<AdminProductListingSearchResult, AdminProductListingError>;
}
pub struct SearchAdminProductListingsHandler<R, A> {
    reader: R,
    check_user_admin: A,
}
impl<R, A> SearchAdminProductListingsHandler<R, A> {
    pub fn new(reader: R, check_user_admin: A) -> Self {
        Self {
            reader,
            check_user_admin,
        }
    }
}
#[async_trait::async_trait]
impl<R: AdminProductListingSearchReader, A: CheckUserAdminUseCase> SearchAdminProductListingsUseCase
    for SearchAdminProductListingsHandler<R, A>
{
    #[tracing::instrument(name = "search_admin_product_listings", skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: AdminProductListingSearchRequest,
    ) -> Result<AdminProductListingSearchResult, AdminProductListingError> {
        authorize_admin(
            context,
            &self.check_user_admin,
            CredentialCapability::ProductListingsRead,
        )
        .await?;
        if !(1..=100).contains(&request.cursor.size) {
            return Err(AdminProductListingError::InvalidPageSize);
        }
        if request
            .scope
            .query
            .as_ref()
            .is_some_and(|q| q.contains('\0') || q.len() > 512)
        {
            return Err(AdminProductListingError::InvalidQuery);
        }
        if request
            .cursor
            .search_after
            .as_ref()
            .is_some_and(|after| after.scope != request.scope)
        {
            return Err(AdminProductListingError::CursorScopeMismatch);
        }
        self.reader.search(&request).await.map_err(Into::into)
    }
}

impl From<AdminProductListingReadError> for AdminProductListingError {
    fn from(error: AdminProductListingReadError) -> Self {
        match error {
            AdminProductListingReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AdminProductListingReadError::InvalidReadModel { source } => {
                Self::InvalidPersistedState { source }
            }
            AdminProductListingReadError::QueryFailed { source } => Self::Internal { source },
        }
    }
}
