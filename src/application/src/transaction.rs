#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TransactionError {
    #[error("failed to begin transaction")]
    BeginFailed,
    #[error("failed to commit transaction")]
    CommitFailed,
}

#[async_trait::async_trait]
pub trait Transaction: Send {
    async fn commit(self) -> Result<(), TransactionError>;
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

        assert_eq!(Ok(()), result);
        let committed = committed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(*committed);
    }
}
