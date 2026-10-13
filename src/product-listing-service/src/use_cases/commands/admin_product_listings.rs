use super::{
    product_listing_management::{Action, ProductListingManagementMutation},
    update_product_listing::UpdateProductListingCommand,
};
use crate::{
    ports::{
        ProductListingEventAppenderFactory, ProductListingRepositoryFactory,
        ProductListingStorageVersion,
    },
    use_cases::admin_product_listing_error::{AdminProductListingError, authorize_admin},
};
use application::{
    operation_context::{CredentialCapability, OperationContext},
    transaction::UnitOfWork,
};
use auction_service::ports::AuctionReferenceValidatorFactory;
use domain_primitives::change_outcome::ChangeOutcome;
use product_listing_core::product_listing_id::ProductListingId;
use user_service::use_cases::queries::check_user_admin::CheckUserAdminUseCase;

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateAdminProductListingCommand {
    pub product_listing_id: ProductListingId,
    pub expected_version: ProductListingStorageVersion,
    pub patch: UpdateProductListingCommand,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdminProductListingLifecycleCommand {
    pub product_listing_id: ProductListingId,
    pub expected_version: ProductListingStorageVersion,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdminProductListingMutationResult {
    pub product_listing_id: ProductListingId,
    pub expected_version: ProductListingStorageVersion,
    pub outcome: ChangeOutcome,
}

#[async_trait::async_trait]
pub trait UpdateAdminProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateAdminProductListingCommand,
    ) -> Result<AdminProductListingMutationResult, AdminProductListingError>;
}
#[async_trait::async_trait]
pub trait WithdrawAdminProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: AdminProductListingLifecycleCommand,
    ) -> Result<AdminProductListingMutationResult, AdminProductListingError>;
}
#[async_trait::async_trait]
pub trait RestoreAdminProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: AdminProductListingLifecycleCommand,
    ) -> Result<AdminProductListingMutationResult, AdminProductListingError>;
}

pub struct AdminProductListingMutationHandler<U, R, E, A, V> {
    mutation: ProductListingManagementMutation<U, R, E, V>,
    check_user_admin: A,
}
impl<U, R, E, A, V> AdminProductListingMutationHandler<U, R, E, A, V> {
    pub fn new(
        unit_of_work: U,
        products: R,
        events: E,
        check_user_admin: A,
        auction_references: V,
    ) -> Self {
        Self {
            mutation: ProductListingManagementMutation::new(
                unit_of_work,
                products,
                events,
                auction_references,
            ),
            check_user_admin,
        }
    }
}

impl<U, R, E, A, V> AdminProductListingMutationHandler<U, R, E, A, V>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: CheckUserAdminUseCase,
    V: AuctionReferenceValidatorFactory<U::Tx>,
{
    #[tracing::instrument(name = "mutate_admin_product_listing", skip_all, fields(product_listing_id = %id, expected_version = %expected_version, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn mutate(
        &self,
        context: &OperationContext,
        id: ProductListingId,
        expected_version: ProductListingStorageVersion,
        action: Action,
    ) -> Result<AdminProductListingMutationResult, AdminProductListingError> {
        authorize_admin(
            context,
            &self.check_user_admin,
            CredentialCapability::ProductListingsWrite,
        )
        .await?;
        self.mutation
            .mutate(
                context,
                id,
                expected_version,
                action,
                None,
                "admin.product_listing.mutated",
            )
            .await
    }
}

#[async_trait::async_trait]
impl<U, R, E, A, V> UpdateAdminProductListingUseCase
    for AdminProductListingMutationHandler<U, R, E, A, V>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: CheckUserAdminUseCase,
    V: AuctionReferenceValidatorFactory<U::Tx>,
{
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateAdminProductListingCommand,
    ) -> Result<AdminProductListingMutationResult, AdminProductListingError> {
        self.mutate(
            context,
            command.product_listing_id,
            command.expected_version,
            Action::Update(Box::new(command.patch)),
        )
        .await
    }
}
#[async_trait::async_trait]
impl<U, R, E, A, V> WithdrawAdminProductListingUseCase
    for AdminProductListingMutationHandler<U, R, E, A, V>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: CheckUserAdminUseCase,
    V: AuctionReferenceValidatorFactory<U::Tx>,
{
    async fn execute(
        &self,
        context: &OperationContext,
        command: AdminProductListingLifecycleCommand,
    ) -> Result<AdminProductListingMutationResult, AdminProductListingError> {
        self.mutate(
            context,
            command.product_listing_id,
            command.expected_version,
            Action::Withdraw,
        )
        .await
    }
}
#[async_trait::async_trait]
impl<U, R, E, A, V> RestoreAdminProductListingUseCase
    for AdminProductListingMutationHandler<U, R, E, A, V>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: CheckUserAdminUseCase,
    V: AuctionReferenceValidatorFactory<U::Tx>,
{
    async fn execute(
        &self,
        context: &OperationContext,
        command: AdminProductListingLifecycleCommand,
    ) -> Result<AdminProductListingMutationResult, AdminProductListingError> {
        self.mutate(
            context,
            command.product_listing_id,
            command.expected_version,
            Action::Restore,
        )
        .await
    }
}
