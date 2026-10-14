//! Durable admission and fenced checkpoints around external effects. Payload codecs belong to services.
use crate::{
    error::box_error,
    idempotency::{CommandIntent, CommandReceiptError},
    transaction::{Transaction, UnitOfWork},
};
use serde_json::Value;
use time::OffsetDateTime;

pub struct ExternalAttempt {
    pub state: Value,
    pub attempt: i64,
    pub admitted_at: OffsetDateTime,
    pub completed: bool,
}
#[async_trait::async_trait]
pub trait ExternalOperations: Send + Sync {
    async fn claim(
        &self,
        intent: &CommandIntent,
        initial: Value,
        exclusive_scope: &str,
        resumable: bool,
    ) -> Result<ExternalAttempt, CommandReceiptError>;
    async fn checkpoint(
        &self,
        intent: &CommandIntent,
        attempt: i64,
        state: Value,
        completed: bool,
    ) -> Result<(), CommandReceiptError>;
}
pub struct TransactionalExternalOperations<U>(pub U);
#[async_trait::async_trait]
impl<U: UnitOfWork> ExternalOperations for TransactionalExternalOperations<U> {
    async fn claim(
        &self,
        intent: &CommandIntent,
        initial: Value,
        exclusive_scope: &str,
        resumable: bool,
    ) -> Result<ExternalAttempt, CommandReceiptError> {
        let mut tx = self
            .0
            .begin()
            .await
            .map_err(|e| CommandReceiptError::Storage(box_error(e)))?;
        let result = tx
            .claim_external_operation(intent, initial, exclusive_scope, resumable)
            .await
            .map_err(|e| crate::idempotency::observe(intent.operation, e))?;
        tx.commit()
            .await
            .map_err(|e| CommandReceiptError::Storage(box_error(e)))?;
        Ok(result)
    }
    async fn checkpoint(
        &self,
        intent: &CommandIntent,
        attempt: i64,
        state: Value,
        completed: bool,
    ) -> Result<(), CommandReceiptError> {
        let mut tx = self
            .0
            .begin()
            .await
            .map_err(|e| CommandReceiptError::Storage(box_error(e)))?;
        tx.checkpoint_external_operation(intent, attempt, state, completed)
            .await?;
        tx.commit()
            .await
            .map_err(|e| CommandReceiptError::Storage(box_error(e)))
    }
}
