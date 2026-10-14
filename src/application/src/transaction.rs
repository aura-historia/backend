#[derive(Debug, thiserror::Error)]
pub enum TransactionError {
    #[error("failed to begin transaction")]
    BeginFailed(#[source] crate::error::BoxError),
    #[error("failed to commit transaction")]
    CommitFailed(#[source] crate::error::BoxError),
}

#[async_trait::async_trait]
pub trait Transaction: Send {
    async fn commit(self) -> Result<(), TransactionError>;

    /// Optional command evidence, in the same transaction as the use case's authoritative writes.
    /// Unsupported transaction implementations fail closed when a command key is supplied.
    async fn command_receipt(
        &mut self,
        _intent: &crate::idempotency::CommandIntent,
    ) -> Result<Option<serde_json::Value>, crate::idempotency::CommandReceiptError> {
        Err(crate::idempotency::CommandReceiptError::Storage(
            crate::error::static_error("command receipts unavailable"),
        ))
    }

    async fn cleanup_command_receipts(
        &mut self,
        _limit: u16,
    ) -> Result<(u64, u64), crate::idempotency::CommandReceiptError> {
        Err(crate::idempotency::CommandReceiptError::Storage(
            crate::error::static_error("command receipt cleanup unavailable"),
        ))
    }

    async fn claim_external_operation(
        &mut self,
        _intent: &crate::idempotency::CommandIntent,
        _initial: serde_json::Value,
        _scope: &str,
        _resumable: bool,
    ) -> Result<crate::external_operation::ExternalAttempt, crate::idempotency::CommandReceiptError>
    {
        Err(crate::idempotency::CommandReceiptError::Storage(
            crate::error::static_error("external operations unavailable"),
        ))
    }
    async fn checkpoint_external_operation(
        &mut self,
        _intent: &crate::idempotency::CommandIntent,
        _attempt: i64,
        _state: serde_json::Value,
        _completed: bool,
    ) -> Result<(), crate::idempotency::CommandReceiptError> {
        Err(crate::idempotency::CommandReceiptError::Storage(
            crate::error::static_error("external operations unavailable"),
        ))
    }

    /// Forget resource-linked content while retaining successful-command identity.
    /// Implementations supporting command receipts must erase the linked evidence here.
    async fn forget_commands(
        &mut self,
        _resource: &crate::idempotency::CommandResource,
    ) -> Result<(), crate::idempotency::CommandReceiptError> {
        Ok(())
    }

    async fn complete_command(
        &mut self,
        _intent: &crate::idempotency::CommandIntent,
        _outcome: serde_json::Value,
    ) -> Result<(), crate::idempotency::CommandReceiptError> {
        Err(crate::idempotency::CommandReceiptError::Storage(
            crate::error::static_error("command receipts unavailable"),
        ))
    }
}

#[async_trait::async_trait]
pub trait UnitOfWork: Send + Sync {
    type Tx: Transaction;

    async fn begin(&self) -> Result<Self::Tx, TransactionError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct TestTransaction {
        committed: Arc<Mutex<bool>>,
    }

    struct TestUnitOfWork {
        committed: Arc<Mutex<bool>>,
    }

    #[async_trait::async_trait]
    impl Transaction for TestTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut committed = self
                .committed
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *committed = true;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for TestUnitOfWork {
        type Tx = TestTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            Ok(TestTransaction {
                committed: Arc::clone(&self.committed),
            })
        }
    }

    #[test]
    fn transaction_errors_preserve_the_original_cause() {
        use std::error::Error;
        for error in [
            TransactionError::BeginFailed(crate::error::static_error("begin cause")),
            TransactionError::CommitFailed(crate::error::static_error("commit cause")),
        ] {
            assert!(
                error
                    .source()
                    .unwrap()
                    .downcast_ref::<crate::error::StaticError>()
                    .is_some()
            );
        }
    }

    #[tokio::test]
    async fn should_begin_and_commit_transaction() {
        let committed = Arc::new(Mutex::new(false));
        let unit_of_work = TestUnitOfWork {
            committed: Arc::clone(&committed),
        };

        let result = match unit_of_work.begin().await {
            Ok(tx) => tx.commit().await,
            Err(error) => Err(error),
        };

        assert!(result.is_ok());
        let committed = committed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(*committed);
    }
}
