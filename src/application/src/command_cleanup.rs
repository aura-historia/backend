//! Bounded, recoverable response erasure. Successful-command fences are never expired.
use crate::{
    idempotency::CommandReceiptError,
    operation_context::{OperationContext, Principal},
    transaction::{Transaction, UnitOfWork},
};
pub struct CommandReceiptCleanupHandler<U>(pub U);
impl<U: UnitOfWork> CommandReceiptCleanupHandler<U> {
    pub async fn execute(
        &self,
        context: &OperationContext,
        limit: u16,
    ) -> Result<(u64, u64), CommandReceiptError> {
        if !matches!(context.principal, Principal::System | Principal::Service(_))
            || !(1..=1000).contains(&limit)
        {
            return Err(CommandReceiptError::InvalidPersistedState(
                crate::error::static_error("invalid command cleanup invocation"),
            ));
        }
        let mut tx = self
            .0
            .begin()
            .await
            .map_err(|e| CommandReceiptError::Storage(Box::new(e)))?;
        let counts = tx.cleanup_command_receipts(limit).await?;
        tx.commit()
            .await
            .map_err(|e| CommandReceiptError::Storage(Box::new(e)))?;
        Ok(counts)
    }
}
