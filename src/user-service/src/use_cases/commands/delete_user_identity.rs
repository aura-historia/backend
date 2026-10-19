use crate::ports::user_identity_deletion::{
    UserIdentityDeleter, UserIdentityDeletionClaimOutcome, UserIdentityDeletionError,
    UserIdentityDeletionStore, UserIdentityDeletionStoreFactory,
};
use application::transaction::{Transaction, UnitOfWork};
use user_core::user_id::UserId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteUserIdentityResult {
    Completed,
    Deferred,
}
#[async_trait::async_trait]
pub trait DeleteUserIdentityUseCase: Send + Sync {
    async fn execute(
        &self,
        user_id: UserId,
    ) -> Result<DeleteUserIdentityResult, UserIdentityDeletionError>;
}
pub struct DeleteUserIdentityHandler<U, S, D> {
    unit_of_work: U,
    store: S,
    deleter: D,
}
impl<U, S, D> DeleteUserIdentityHandler<U, S, D> {
    pub fn new(unit_of_work: U, store: S, deleter: D) -> Self {
        Self {
            unit_of_work,
            store,
            deleter,
        }
    }
}
#[async_trait::async_trait]
impl<U, S, D> DeleteUserIdentityUseCase for DeleteUserIdentityHandler<U, S, D>
where
    U: UnitOfWork,
    S: UserIdentityDeletionStoreFactory<U::Tx>,
    D: UserIdentityDeleter,
{
    #[tracing::instrument(name = "delete_user_identity", skip_all, fields(user_id = %user_id))]
    async fn execute(
        &self,
        user_id: UserId,
    ) -> Result<DeleteUserIdentityResult, UserIdentityDeletionError> {
        let mut tx = self.unit_of_work.begin().await.map_err(transaction_error)?;
        let outcome = self.store.in_transaction(&mut tx).claim(user_id).await?;
        tx.commit().await.map_err(transaction_error)?;
        let claim = match outcome {
            UserIdentityDeletionClaimOutcome::Completed => {
                return Ok(DeleteUserIdentityResult::Completed);
            }
            UserIdentityDeletionClaimOutcome::Deferred => {
                return Ok(DeleteUserIdentityResult::Deferred);
            }
            UserIdentityDeletionClaimOutcome::Claimed(claim) => claim,
        };
        let username = if let Some(username) = claim.username.clone() {
            username
        } else {
            let username = self
                .deleter
                .resolve_username(&claim.identity)
                .await?
                .ok_or_else(|| {
                    UserIdentityDeletionError(application::error::static_error(
                        "identity deletion lookup not yet observed",
                    ))
                })?;
            let mut tx = self.unit_of_work.begin().await.map_err(transaction_error)?;
            self.store
                .in_transaction(&mut tx)
                .save_username(&claim, &username)
                .await?;
            tx.commit().await.map_err(transaction_error)?;
            username
        };
        // The exact target survives lost provider responses and completion commits.
        // No authoritative transaction spans a provider call.
        self.deleter
            .delete_identity(&claim.identity, &username)
            .await?;
        let mut tx = self.unit_of_work.begin().await.map_err(transaction_error)?;
        self.store.in_transaction(&mut tx).complete(&claim).await?;
        tx.commit().await.map_err(transaction_error)?;
        tracing::info!(event = "user.identity_deletion.completed", user_id = %user_id);
        Ok(DeleteUserIdentityResult::Completed)
    }
}
fn transaction_error(
    source: application::transaction::TransactionError,
) -> UserIdentityDeletionError {
    UserIdentityDeletionError(Box::new(source))
}
