use super::admin_product_listings::validate_search;
use crate::{
    ports::admin_product_listing_reader::*,
    use_cases::{
        admin_product_listing_error::AdminProductListingError,
        partner_product_listing_error::{PartnerProductListingError, authorize_partner},
    },
};
use application::operation_context::{CredentialCapability, OperationContext};
use listing_source_core::ListingSourceId;
use partnership_service::ports::ListingSourceAuthorization;
use product_listing_core::product_listing_id::ProductListingId;

#[async_trait::async_trait]
pub trait GetPartnerProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        source: ListingSourceId,
        id: ProductListingId,
    ) -> Result<AdminProductListingDetails, PartnerProductListingError>;
}

pub struct GetPartnerProductListingHandler<R, A> {
    reader: R,
    authorization: A,
}
impl<R, A> GetPartnerProductListingHandler<R, A> {
    pub fn new(reader: R, authorization: A) -> Self {
        Self {
            reader,
            authorization,
        }
    }
}
#[async_trait::async_trait]
impl<R: AdminProductListingDetailsReader, A: ListingSourceAuthorization>
    GetPartnerProductListingUseCase for GetPartnerProductListingHandler<R, A>
{
    #[tracing::instrument(name = "get_partner_product_listing", skip_all, fields(listing_source_id = %source, product_listing_id = %id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        source: ListingSourceId,
        id: ProductListingId,
    ) -> Result<AdminProductListingDetails, PartnerProductListingError> {
        authorize_partner(
            context,
            &self.authorization,
            source,
            CredentialCapability::ProductListingsRead,
        )
        .await?;
        self.reader
            .find_by_id_in_source(source, id)
            .await
            .map_err(AdminProductListingError::from)?
            .filter(|listing| listing.summary.source.listing_source_id == source)
            .ok_or_else(|| AdminProductListingError::NotFound.into())
    }
}

#[async_trait::async_trait]
pub trait ListPartnerProductListingsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        source: ListingSourceId,
        request: AdminProductListingSearchRequest,
    ) -> Result<AdminProductListingSearchResult, PartnerProductListingError>;
}

pub struct ListPartnerProductListingsHandler<R, A> {
    reader: R,
    authorization: A,
}
impl<R, A> ListPartnerProductListingsHandler<R, A> {
    pub fn new(reader: R, authorization: A) -> Self {
        Self {
            reader,
            authorization,
        }
    }
}
#[async_trait::async_trait]
impl<R: AdminProductListingSearchReader, A: ListingSourceAuthorization>
    ListPartnerProductListingsUseCase for ListPartnerProductListingsHandler<R, A>
{
    #[tracing::instrument(name = "list_partner_product_listings", skip_all, fields(listing_source_id = %source, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        source: ListingSourceId,
        mut request: AdminProductListingSearchRequest,
    ) -> Result<AdminProductListingSearchResult, PartnerProductListingError> {
        authorize_partner(
            context,
            &self.authorization,
            source,
            CredentialCapability::ProductListingsRead,
        )
        .await?;
        // The path always owns the source scope, including for non-HTTP callers.
        request.scope.listing_source_id = Some(source);
        validate_search(&request)?;
        self.reader
            .search(&request)
            .await
            .map_err(AdminProductListingError::from)
            .map_err(Into::into)
    }
}
