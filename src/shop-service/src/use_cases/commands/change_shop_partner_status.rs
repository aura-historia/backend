use crate::ports::{ShopRepository, ShopRepositoryError, ShopRepositoryFactory};
use common::operation_context::OperationContext;
use common::transaction::{Transaction, UnitOfWork};
use common::write_metadata::WriteMetadata;
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
    TemporarilyUnavailable,
    #[error("invalid persisted shop state")]
    InvalidPersistedState,
    #[error("internal shop persistence failure")]
    Internal,
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
        let metadata = WriteMetadata::try_from(context)
            .map_err(|_| ChangeShopPartnerStatusError::AuthenticatedActorRequired)?;
        tracing::Span::current().record("actor_id", tracing::field::display(metadata.actor()));

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ChangeShopPartnerStatusError::BeginTransactionFailed)?;

        let loaded = self
            .shops
            .in_transaction(&mut tx)
            .find_by_id(command.shop_id)
            .await?
            .ok_or(ChangeShopPartnerStatusError::ShopNotFound)?;

        let common::versioned::Versioned {
            value: mut shop,
            version,
        } = loaded;

        let outcome = shop.change_partner_status(command.partner_status);
        if outcome.changed() {
            self.shops
                .in_transaction(&mut tx)
                .update(&shop, version, &metadata)
                .await?;
        }

        tx.commit()
            .await
            .map_err(|_| ChangeShopPartnerStatusError::CommitTransactionFailed)?;

        tracing::info!(
            event = "shop.partner_status_changed",
            actor_type = context.principal.kind(),
            actor_id = %metadata.actor(),
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
            ShopRepositoryError::TemporarilyUnavailable => Self::TemporarilyUnavailable,
            ShopRepositoryError::InvalidPersistedState => Self::InvalidPersistedState,
            ShopRepositoryError::SlugConflict | ShopRepositoryError::Internal => Self::Internal,
        }
    }
}
