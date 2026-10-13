use super::{
    admin_product_listings::AdminProductListingMutationResult,
    product_listing_management::{Action, ProductListingManagementMutation},
    update_product_listing::UpdateProductListingCommand,
};
use crate::{
    ports::{
        ProductListingEventAppenderFactory, ProductListingRepositoryFactory,
        ProductListingStorageVersion,
    },
    use_cases::partner_product_listing_error::{PartnerProductListingError, authorize_partner},
};
use application::{
    operation_context::{CredentialCapability, OperationContext},
    transaction::UnitOfWork,
};
use auction_service::ports::AuctionReferenceValidatorFactory;
use listing_source_core::ListingSourceId;
use partnership_service::ports::ListingSourceAuthorization;
use product_listing_core::product_listing_id::ProductListingId;

#[derive(Debug, Clone, PartialEq)]
pub struct UpdatePartnerProductListingCommand {
    pub listing_source_id: ListingSourceId,
    pub product_listing_id: ProductListingId,
    pub expected_version: ProductListingStorageVersion,
    pub patch: UpdateProductListingCommand,
}
#[derive(Debug, Clone, PartialEq)]
pub struct PartnerProductListingLifecycleCommand {
    pub listing_source_id: ListingSourceId,
    pub product_listing_id: ProductListingId,
    pub expected_version: ProductListingStorageVersion,
}

#[async_trait::async_trait]
pub trait UpdatePartnerProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdatePartnerProductListingCommand,
    ) -> Result<AdminProductListingMutationResult, PartnerProductListingError>;
}
#[async_trait::async_trait]
pub trait WithdrawPartnerProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: PartnerProductListingLifecycleCommand,
    ) -> Result<AdminProductListingMutationResult, PartnerProductListingError>;
}
#[async_trait::async_trait]
pub trait RestorePartnerProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: PartnerProductListingLifecycleCommand,
    ) -> Result<AdminProductListingMutationResult, PartnerProductListingError>;
}

pub struct PartnerProductListingMutationHandler<U, R, E, A, V> {
    mutation: ProductListingManagementMutation<U, R, E, V>,
    authorization: A,
}
impl<U, R, E, A, V> PartnerProductListingMutationHandler<U, R, E, A, V> {
    pub fn new(
        unit_of_work: U,
        products: R,
        events: E,
        authorization: A,
        auction_references: V,
    ) -> Self {
        Self {
            mutation: ProductListingManagementMutation::new(
                unit_of_work,
                products,
                events,
                auction_references,
            ),
            authorization,
        }
    }
}
impl<U, R, E, A, V> PartnerProductListingMutationHandler<U, R, E, A, V>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: ListingSourceAuthorization,
    V: AuctionReferenceValidatorFactory<U::Tx>,
{
    #[tracing::instrument(name = "mutate_partner_product_listing", skip_all, fields(listing_source_id = %source, product_listing_id = %id, expected_version = %expected_version, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn mutate(
        &self,
        context: &OperationContext,
        source: ListingSourceId,
        id: ProductListingId,
        expected_version: ProductListingStorageVersion,
        action: Action,
    ) -> Result<AdminProductListingMutationResult, PartnerProductListingError> {
        authorize_partner(
            context,
            &self.authorization,
            source,
            CredentialCapability::ProductListingsWrite,
        )
        .await?;
        self.mutation
            .mutate(
                context,
                id,
                expected_version,
                action,
                Some(source),
                "partner.product_listing.mutated",
            )
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl<U, R, E, A, V> UpdatePartnerProductListingUseCase
    for PartnerProductListingMutationHandler<U, R, E, A, V>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: ListingSourceAuthorization,
    V: AuctionReferenceValidatorFactory<U::Tx>,
{
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdatePartnerProductListingCommand,
    ) -> Result<AdminProductListingMutationResult, PartnerProductListingError> {
        self.mutate(
            context,
            command.listing_source_id,
            command.product_listing_id,
            command.expected_version,
            Action::Update(Box::new(command.patch)),
        )
        .await
    }
}
#[async_trait::async_trait]
impl<U, R, E, A, V> WithdrawPartnerProductListingUseCase
    for PartnerProductListingMutationHandler<U, R, E, A, V>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: ListingSourceAuthorization,
    V: AuctionReferenceValidatorFactory<U::Tx>,
{
    async fn execute(
        &self,
        context: &OperationContext,
        command: PartnerProductListingLifecycleCommand,
    ) -> Result<AdminProductListingMutationResult, PartnerProductListingError> {
        self.mutate(
            context,
            command.listing_source_id,
            command.product_listing_id,
            command.expected_version,
            Action::Withdraw,
        )
        .await
    }
}
#[async_trait::async_trait]
impl<U, R, E, A, V> RestorePartnerProductListingUseCase
    for PartnerProductListingMutationHandler<U, R, E, A, V>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: ListingSourceAuthorization,
    V: AuctionReferenceValidatorFactory<U::Tx>,
{
    async fn execute(
        &self,
        context: &OperationContext,
        command: PartnerProductListingLifecycleCommand,
    ) -> Result<AdminProductListingMutationResult, PartnerProductListingError> {
        self.mutate(
            context,
            command.listing_source_id,
            command.product_listing_id,
            command.expected_version,
            Action::Restore,
        )
        .await
    }
}
