use crate::ports::{
    ProductSimilarityReadError, ProductSimilarityReader, ProductSimilarityReaderFactory,
};
use common::error::boxed::BoxError;
use common::product_id::ProductKey;
use common::transaction::{Transaction, UnitOfWork};

#[derive(Debug, Clone, PartialEq)]
pub struct GetSimilarProductsRequest {
    pub product_key: ProductKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GetSimilarProductsResult {
    EmbeddingPending,
}

#[derive(Debug, thiserror::Error)]
pub enum GetSimilarProductsError {
    #[error("product not found")]
    NotFound,
    #[error("product similarity seed query failed")]
    ProductSimilarityQueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("similarity search is unavailable")]
    SimilaritySearchUnavailable,
    #[error("failed to begin get similar products transaction")]
    BeginTransactionFailed,
    #[error("failed to commit get similar products transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait GetSimilarProductsUseCase: Send + Sync {
    async fn execute(
        &self,
        request: GetSimilarProductsRequest,
    ) -> Result<GetSimilarProductsResult, GetSimilarProductsError>;
}

pub struct GetSimilarProductsHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> GetSimilarProductsHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> GetSimilarProductsUseCase for GetSimilarProductsHandler<U, R>
where
    U: UnitOfWork,
    R: ProductSimilarityReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "get_similar_products",
        skip_all,
        fields(
            shop_id = %request.product_key.shop_id,
            shops_product_id = %request.product_key.shops_product_id,
        )
    )]
    async fn execute(
        &self,
        request: GetSimilarProductsRequest,
    ) -> Result<GetSimilarProductsResult, GetSimilarProductsError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetSimilarProductsError::BeginTransactionFailed)?;
        let seed = self
            .reader
            .in_transaction(&mut tx)
            .find_seed(&request.product_key)
            .await?
            .ok_or(GetSimilarProductsError::NotFound)?;

        if seed.embedding.is_some() {
            return Err(GetSimilarProductsError::SimilaritySearchUnavailable);
        }

        tx.commit()
            .await
            .map_err(|_| GetSimilarProductsError::CommitTransactionFailed)?;

        Ok(GetSimilarProductsResult::EmbeddingPending)
    }
}

impl From<ProductSimilarityReadError> for GetSimilarProductsError {
    fn from(error: ProductSimilarityReadError) -> Self {
        match error {
            ProductSimilarityReadError::ProductSimilarityQueryFailed { source } => {
                Self::ProductSimilarityQueryFailed { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::ProductSimilaritySeed;
    use common::error::boxed::box_error;
    use common::product_id::ProductId;
    use common::transaction::TransactionError;
    use std::sync::{Arc, Mutex, MutexGuard};

    #[derive(Debug, Default)]
    struct FakeState {
        begin_error: bool,
        commit_error: bool,
        find_seed_result: Option<Result<Option<ProductSimilaritySeed>, ProductSimilarityReadError>>,
        requested_keys: Vec<ProductKey>,
        commit_count: usize,
    }

    type SharedState = Arc<Mutex<FakeState>>;

    #[derive(Clone)]
    struct FakeUnitOfWork {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeSimilarityReaderFactory {
        state: SharedState,
    }

    struct FakeTx {
        state: SharedState,
    }

    struct FakeSimilarityReader {
        state: SharedState,
    }

    fn state() -> SharedState {
        Arc::new(Mutex::new(FakeState::default()))
    }

    fn lock_state(state: &SharedState) -> MutexGuard<'_, FakeState> {
        match state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            if lock_state(&self.state).begin_error {
                Err(TransactionError::BeginFailed)
            } else {
                Ok(FakeTx {
                    state: Arc::clone(&self.state),
                })
            }
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = lock_state(&self.state);
            state.commit_count += 1;
            if state.commit_error {
                Err(TransactionError::CommitFailed)
            } else {
                Ok(())
            }
        }
    }

    impl ProductSimilarityReaderFactory<FakeTx> for FakeSimilarityReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl ProductSimilarityReader + 'tx {
            FakeSimilarityReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductSimilarityReader for FakeSimilarityReader {
        async fn find_seed(
            &mut self,
            product_key: &ProductKey,
        ) -> Result<Option<ProductSimilaritySeed>, ProductSimilarityReadError> {
            let mut state = lock_state(&self.state);
            state.requested_keys.push(product_key.clone());
            match state.find_seed_result.take() {
                Some(result) => result,
                None => Ok(None),
            }
        }
    }

    fn handler(
        state: &SharedState,
    ) -> GetSimilarProductsHandler<FakeUnitOfWork, FakeSimilarityReaderFactory> {
        GetSimilarProductsHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(state),
            },
            FakeSimilarityReaderFactory {
                state: Arc::clone(state),
            },
        )
    }

    fn request() -> GetSimilarProductsRequest {
        GetSimilarProductsRequest {
            product_key: ProductKey::new(
                common::shop_id::ShopId::new(),
                common::shops_product_id::ShopsProductId::new(),
            ),
        }
    }

    #[tokio::test]
    async fn should_return_embedding_pending_when_product_embedding_is_missing() {
        let state = state();
        let request = request();
        lock_state(&state).find_seed_result = Some(Ok(Some(ProductSimilaritySeed {
            product_id: ProductId::new(),
            embedding: None,
        })));

        let result = handler(&state).execute(request.clone()).await;

        assert!(matches!(
            result,
            Ok(GetSimilarProductsResult::EmbeddingPending)
        ));
        assert_eq!(vec![request.product_key], lock_state(&state).requested_keys);
        assert_eq!(1, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_return_not_found_when_product_key_is_missing() {
        let state = state();

        let result = handler(&state).execute(request()).await;

        assert!(matches!(result, Err(GetSimilarProductsError::NotFound)));
        assert_eq!(0, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_not_report_pending_when_embedding_is_available() {
        let state = state();
        lock_state(&state).find_seed_result = Some(Ok(Some(ProductSimilaritySeed {
            product_id: ProductId::new(),
            embedding: Some(vec![0.1_f32]),
        })));

        let result = handler(&state).execute(request()).await;

        assert!(matches!(
            result,
            Err(GetSimilarProductsError::SimilaritySearchUnavailable)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_map_similarity_seed_query_failure() {
        let state = state();
        lock_state(&state).find_seed_result = Some(Err(
            ProductSimilarityReadError::ProductSimilarityQueryFailed {
                source: box_error(std::io::Error::other("boom")),
            },
        ));

        let result = handler(&state).execute(request()).await;

        assert!(matches!(
            result,
            Err(GetSimilarProductsError::ProductSimilarityQueryFailed { .. })
        ));
        assert_eq!(0, lock_state(&state).commit_count);
    }
}
