use crate::ports::{
    PartnerShopRepository, PartnerShopRepositoryError, PartnerShopRepositoryFactory,
    ShopRepository, ShopRepositoryError, ShopRepositoryFactory,
};
use common::operation_context::OperationContext;
use common::transaction::{Transaction, UnitOfWork};
use common::write_metadata::WriteMetadata;
use common::{shop_id::ShopId, user_id::UserId};

#[derive(Debug, Clone, PartialEq)]
pub struct GrantPartnerShopCommand {
    pub user_id: UserId,
    pub shop_id: ShopId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GrantPartnerShopResult {
    pub user_id: UserId,
    pub shop_id: ShopId,
}

#[derive(Debug, thiserror::Error)]
pub enum GrantPartnerShopError {
    #[error("authenticated actor required to grant partner shop")]
    AuthenticatedActorRequired,
    #[error("user not found")]
    UserNotFound,
    #[error("shop not found")]
    ShopNotFound,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary partner shop persistence failure")]
    TemporarilyUnavailable,
    #[error("invalid persisted shop state")]
    InvalidPersistedState,
    #[error("internal partner shop persistence failure")]
    Internal,
    #[error("failed to begin grant partner shop transaction")]
    BeginTransactionFailed,
    #[error("failed to commit grant partner shop transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait GrantPartnerShopUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: GrantPartnerShopCommand,
    ) -> Result<GrantPartnerShopResult, GrantPartnerShopError>;
}

pub struct GrantPartnerShopHandler<U, S, P> {
    unit_of_work: U,
    shops: S,
    partner_shops: P,
}

impl<U, S, P> GrantPartnerShopHandler<U, S, P> {
    pub fn new(unit_of_work: U, shops: S, partner_shops: P) -> Self {
        Self {
            unit_of_work,
            shops,
            partner_shops,
        }
    }
}

#[async_trait::async_trait]
impl<U, S, P> GrantPartnerShopUseCase for GrantPartnerShopHandler<U, S, P>
where
    U: UnitOfWork,
    S: ShopRepositoryFactory<U::Tx>,
    P: PartnerShopRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "grant_partner_shop",
        skip_all,
        fields(
            user_id = %command.user_id,
            shop_id = %command.shop_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: GrantPartnerShopCommand,
    ) -> Result<GrantPartnerShopResult, GrantPartnerShopError> {
        let metadata = WriteMetadata::try_from(context)
            .map_err(|_| GrantPartnerShopError::AuthenticatedActorRequired)?;
        tracing::Span::current().record("actor_id", tracing::field::display(metadata.actor()));

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GrantPartnerShopError::BeginTransactionFailed)?;

        self.shops
            .in_transaction(&mut tx)
            .find_by_id(command.shop_id)
            .await?
            .ok_or(GrantPartnerShopError::ShopNotFound)?;

        self.partner_shops
            .in_transaction(&mut tx)
            .grant(command.user_id, command.shop_id, &metadata)
            .await?;

        tx.commit()
            .await
            .map_err(|_| GrantPartnerShopError::CommitTransactionFailed)?;

        tracing::info!(
            event = "shop.partner_granted",
            actor_type = context.principal.kind(),
            actor_id = %metadata.actor(),
            user_id = %command.user_id,
            shop_id = %command.shop_id,
            outcome = "success",
        );

        Ok(GrantPartnerShopResult {
            user_id: command.user_id,
            shop_id: command.shop_id,
        })
    }
}

impl From<ShopRepositoryError> for GrantPartnerShopError {
    fn from(error: ShopRepositoryError) -> Self {
        match error {
            ShopRepositoryError::TemporarilyUnavailable => Self::TemporarilyUnavailable,
            ShopRepositoryError::InvalidPersistedState => Self::InvalidPersistedState,
            ShopRepositoryError::ConcurrencyConflict
            | ShopRepositoryError::SlugConflict
            | ShopRepositoryError::Internal => Self::Internal,
        }
    }
}

impl From<PartnerShopRepositoryError> for GrantPartnerShopError {
    fn from(error: PartnerShopRepositoryError) -> Self {
        match error {
            PartnerShopRepositoryError::UserNotFound => Self::UserNotFound,
            PartnerShopRepositoryError::ShopNotFound => Self::ShopNotFound,
            PartnerShopRepositoryError::TemporarilyUnavailable => Self::TemporarilyUnavailable,
            PartnerShopRepositoryError::Internal => Self::Internal,
        }
    }
}
