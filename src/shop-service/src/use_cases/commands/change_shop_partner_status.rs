use crate::ports::{ShopRepository, ShopRepositoryError, ShopRepositoryFactory};
use common::error::boxed::BoxError;
use common::operation_context::OperationContext;
use common::transaction::{Transaction, UnitOfWork};
use common::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};
use shop_core::{partner_status::ShopPartnerStatus, shop::Shop};

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeShopPartnerStatusCommand {
    pub shop_id: ShopId,
    pub partner_status: ShopPartnerStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeShopPartnerStatusResult {
    pub shop_id: ShopId,
    pub shop_slug_id: ShopSlugId,
    pub name: ShopName,
    pub partner_status: ShopPartnerStatus,
}

#[derive(Debug, thiserror::Error)]
pub enum ChangeShopPartnerStatusError {
    #[error("authenticated actor required to change shop partner status")]
    AuthenticatedActorRequired,
    #[error("shop not found")]
    ShopNotFound,
    #[error("concurrent shop update")]
    ConcurrencyConflict,
    #[error("temporary shop persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted shop state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal shop persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin change shop partner status transaction")]
    BeginTransactionFailed,
    #[error("failed to commit change shop partner status transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait ChangeShopPartnerStatusUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: ChangeShopPartnerStatusCommand,
    ) -> Result<ChangeShopPartnerStatusResult, ChangeShopPartnerStatusError>;
}

pub struct ChangeShopPartnerStatusHandler<U, R> {
    unit_of_work: U,
    shops: R,
}

impl<U, R> ChangeShopPartnerStatusHandler<U, R> {
    pub fn new(unit_of_work: U, shops: R) -> Self {
        Self {
            unit_of_work,
            shops,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> ChangeShopPartnerStatusUseCase for ChangeShopPartnerStatusHandler<U, R>
where
    U: UnitOfWork,
    R: ShopRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "change_shop_partner_status",
        skip_all,
        fields(
            shop_id = %command.shop_id,
            partner_status = ?command.partner_status,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: ChangeShopPartnerStatusCommand,
    ) -> Result<ChangeShopPartnerStatusResult, ChangeShopPartnerStatusError> {
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ChangeShopPartnerStatusError::BeginTransactionFailed)?;

        let common::versioned::Versioned {
            value: mut shop,
            version,
        } = self
            .shops
            .in_transaction(&mut tx)
            .find_by_id(command.shop_id)
            .await?
            .ok_or(ChangeShopPartnerStatusError::ShopNotFound)?;

        let outcome = shop.change_partner_status(command.partner_status);
        if outcome.changed() {
            self.shops
                .in_transaction(&mut tx)
                .update(&shop, version)
                .await?;
        }

        tx.commit()
            .await
            .map_err(|_| ChangeShopPartnerStatusError::CommitTransactionFailed)?;

        tracing::info!(
            event = "shop.partner_status_changed",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            shop_id = %shop.id(),
            partner_status = ?shop.partner_status(),
            changed = outcome.changed(),
            outcome = "success",
        );

        Ok(ChangeShopPartnerStatusResult::from(&shop))
    }
}

impl From<&Shop> for ChangeShopPartnerStatusResult {
    fn from(shop: &Shop) -> Self {
        Self {
            shop_id: shop.id(),
            shop_slug_id: shop.slug_id().clone(),
            name: shop.name().clone(),
            partner_status: shop.partner_status(),
        }
    }
}

impl From<ShopRepositoryError> for ChangeShopPartnerStatusError {
    fn from(error: ShopRepositoryError) -> Self {
        match error {
            ShopRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            ShopRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ShopRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            ShopRepositoryError::SlugConflict { source }
            | ShopRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}
