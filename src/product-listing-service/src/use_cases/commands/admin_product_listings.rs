use super::{
    product_listing_change::{PersistProductListingChangeError, persist_product_listing_change},
    update_product_listing::{
        ProductListingPatchError, UpdateProductListingCommand, apply_product_listing_update,
    },
};
use crate::{
    ports::{
        ProductListingEventAppenderFactory, ProductListingRepository,
        ProductListingRepositoryError, ProductListingRepositoryFactory,
        ProductListingStorageVersion,
    },
    use_cases::admin_product_listing_error::{AdminProductListingError, authorize_admin},
};
use application::{
    error::box_error,
    operation_context::{CredentialCapability, OperationContext},
    transaction::{Transaction, UnitOfWork},
};
use auction_service::ports::AuctionReferenceValidatorFactory;
use domain_primitives::change_outcome::ChangeOutcome;
use product_listing_core::{
    listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
};
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
    unit_of_work: U,
    products: R,
    events: E,
    check_user_admin: A,
    auction_references: V,
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
            unit_of_work,
            products,
            events,
            check_user_admin,
            auction_references,
        }
    }
}

enum Action {
    Update(Box<UpdateProductListingCommand>),
    Withdraw,
    Restore,
}
impl Action {
    fn name(&self) -> &'static str {
        match self {
            Self::Update(_) => "update",
            Self::Withdraw => "withdraw",
            Self::Restore => "restore",
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
        let action_name = action.name();
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            AdminProductListingError::BeginTransactionFailed(box_error(source))
        })?;
        let loaded = self
            .products
            .in_transaction(&mut tx)
            .find_by_id(id)
            .await?
            .ok_or(AdminProductListingError::NotFound)?;
        if loaded.version != expected_version {
            return Err(AdminProductListingError::ConcurrencyConflict);
        }
        let mut product = loaded.value;
        match action {
            Action::Update(patch) => {
                if product.lifecycle() == ListingLifecycle::Withdrawn {
                    return Err(AdminProductListingError::ListingWithdrawn);
                }
                apply_product_listing_update(
                    &mut tx,
                    &self.auction_references,
                    &mut product,
                    *patch,
                )
                .await
                .map_err(map_patch_error)?;
            }
            Action::Withdraw => {
                product
                    .withdraw()
                    .map_err(|source| AdminProductListingError::InvalidPatch {
                        source: box_error(source),
                    })?;
            }
            Action::Restore => {
                product
                    .restore()
                    .map_err(|source| AdminProductListingError::InvalidPatch {
                        source: box_error(source),
                    })?;
            }
        }
        let persisted = persist_product_listing_change(
            &mut tx,
            &self.products,
            &self.events,
            &mut product,
            loaded.version,
        )
        .await
        .map_err(|error| match error {
            PersistProductListingChangeError::Repository(source) => {
                AdminProductListingError::from(source)
            }
            PersistProductListingChangeError::Event(
                crate::ports::ProductListingEventAppendError::TemporarilyUnavailable { source },
            ) => AdminProductListingError::TemporarilyUnavailable { source },
            PersistProductListingChangeError::Event(source) => AdminProductListingError::Internal {
                source: box_error(source),
            },
        })?;
        tx.commit().await.map_err(|source| {
            AdminProductListingError::CommitTransactionFailed(box_error(source))
        })?;
        tracing::info!(event = "admin.product_listing.mutated", actor_type = context.principal.kind(), actor_id = %context.principal.label(), request_id = %context.request_id, correlation_id = %context.correlation_id, action = action_name, product_listing_id = %id, expected_version = %expected_version, result_version = %persisted.version, event_id = ?persisted.event_id, changed_fields = ?persisted.changed_fields, outcome = if persisted.outcome.changed() { "CHANGED" } else { "UNCHANGED" }, status = "success");
        Ok(AdminProductListingMutationResult {
            product_listing_id: id,
            expected_version: persisted.version,
            outcome: persisted.outcome,
        })
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

fn map_patch_error(error: ProductListingPatchError) -> AdminProductListingError {
    match error {
        ProductListingPatchError::Auction(
            auction_service::ports::AuctionReferenceValidationError::NotFound,
        ) => AdminProductListingError::AuctionNotFound,
        ProductListingPatchError::Auction(
            auction_service::ports::AuctionReferenceValidationError::ListingSourceMismatch,
        ) => AdminProductListingError::AuctionSourceMismatch,
        ProductListingPatchError::Auction(
            auction_service::ports::AuctionReferenceValidationError::TemporarilyUnavailable {
                source,
            },
        ) => AdminProductListingError::TemporarilyUnavailable { source },
        ProductListingPatchError::Auction(
            auction_service::ports::AuctionReferenceValidationError::InvalidPersistedState {
                source,
            },
        ) => AdminProductListingError::InvalidPersistedState { source },
        error => AdminProductListingError::InvalidPatch {
            source: box_error(error),
        },
    }
}
impl From<ProductListingRepositoryError> for AdminProductListingError {
    fn from(error: ProductListingRepositoryError) -> Self {
        match error {
            ProductListingRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            ProductListingRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ProductListingRepositoryError::QueryFailed { source } => Self::Internal { source },
            error => Self::InvalidPersistedState {
                source: box_error(error),
            },
        }
    }
}
