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

/// The deletion step that failed. Each step keeps the port failure as its source.
#[derive(Debug, thiserror::Error)]
pub enum DeleteUserIdentityError {
    #[error("failed to claim user identity deletion")]
    Claim(#[source] UserIdentityDeletionError),
    #[error("failed to look up user identity deletion target")]
    TargetLookup(#[source] UserIdentityDeletionError),
    /// The eventually consistent lookup has not observed the identity yet.
    #[error("user identity deletion target is not yet observable")]
    TargetNotYetObserved,
    #[error("failed to save user identity deletion target")]
    TargetSave(#[source] UserIdentityDeletionError),
    #[error("failed to remove user identity from the identity provider")]
    IdentityRemoval(#[source] UserIdentityDeletionError),
    #[error("failed to complete user identity deletion")]
    Completion(#[source] UserIdentityDeletionError),
}

#[async_trait::async_trait]
pub trait DeleteUserIdentityUseCase: Send + Sync {
    async fn execute(
        &self,
        user_id: UserId,
    ) -> Result<DeleteUserIdentityResult, DeleteUserIdentityError>;
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
    ) -> Result<DeleteUserIdentityResult, DeleteUserIdentityError> {
        use DeleteUserIdentityError as E;
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|source| E::Claim(storage_error(source)))?;
        let outcome = self
            .store
            .in_transaction(&mut tx)
            .claim(user_id)
            .await
            .map_err(E::Claim)?;
        tx.commit()
            .await
            .map_err(|source| E::Claim(storage_error(source)))?;
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
                .await
                .map_err(E::TargetLookup)?
                .ok_or(E::TargetNotYetObserved)?;
            let mut tx = self
                .unit_of_work
                .begin()
                .await
                .map_err(|source| E::TargetSave(storage_error(source)))?;
            self.store
                .in_transaction(&mut tx)
                .save_username(&claim, &username)
                .await
                .map_err(E::TargetSave)?;
            tx.commit()
                .await
                .map_err(|source| E::TargetSave(storage_error(source)))?;
            username
        };
        // The exact target survives lost provider responses and completion commits.
        // No authoritative transaction spans a provider call.
        self.deleter
            .delete_identity(&claim.identity, &username)
            .await
            .map_err(E::IdentityRemoval)?;
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|source| E::Completion(storage_error(source)))?;
        self.store
            .in_transaction(&mut tx)
            .complete(&claim)
            .await
            .map_err(E::Completion)?;
        tx.commit()
            .await
            .map_err(|source| E::Completion(storage_error(source)))?;
        tracing::info!(event = "user.identity_deletion.completed", user_id = %user_id);
        Ok(DeleteUserIdentityResult::Completed)
    }
}
fn storage_error(source: application::transaction::TransactionError) -> UserIdentityDeletionError {
    UserIdentityDeletionError::Storage(Box::new(source))
}
