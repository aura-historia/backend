use crate::ports::{
    ProductEmbeddingLookup, ProductEmbeddingReadError, ProductEmbeddingReader,
    ProductEmbeddingReaderFactory, ProductSimilarProductsReadError, ProductSimilarProductsReader,
    ProductSimilarProductsRequest,
};
use crate::use_cases::ProductSummary;
use common::error::boxed::BoxError;
use common::language::domain::Language;
use common::transaction::{Transaction, UnitOfWork};

#[derive(Debug, Clone, PartialEq)]
pub struct GetSimilarProductsRequest {
    pub lookup: ProductEmbeddingLookup,
    pub language: Language,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GetSimilarProductsResult {
    EmbeddingPending,
    Ready(Vec<ProductSummary>),
}

#[derive(Debug, thiserror::Error)]
pub enum GetSimilarProductsError {
    #[error("product not found")]
    NotFound,
    #[error("product embedding query failed")]
    ProductEmbeddingQueryFailed {
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

pub struct GetSimilarProductsHandler<U, E, S> {
    unit_of_work: U,
    embedding_reader: E,
    similar_products_reader: S,
}

impl<U, E, S> GetSimilarProductsHandler<U, E, S> {
    pub fn new(unit_of_work: U, embedding_reader: E, similar_products_reader: S) -> Self {
        Self {
            unit_of_work,
            embedding_reader,
            similar_products_reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, E, S> GetSimilarProductsUseCase for GetSimilarProductsHandler<U, E, S>
where
    U: UnitOfWork,
    E: ProductEmbeddingReaderFactory<U::Tx>,
    S: ProductSimilarProductsReader,
{
    #[tracing::instrument(name = "get_similar_products", skip_all, fields())]
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
            .embedding_reader
            .in_transaction(&mut tx)
            .find_embedding(&request.lookup)
            .await?
            .ok_or(GetSimilarProductsError::NotFound)?;

        let Some(embedding) = seed.embedding else {
            tx.commit()
                .await
                .map_err(|_| GetSimilarProductsError::CommitTransactionFailed)?;

            return Ok(GetSimilarProductsResult::EmbeddingPending);
        };

        tx.commit()
            .await
            .map_err(|_| GetSimilarProductsError::CommitTransactionFailed)?;

        let products = self
            .similar_products_reader
            .find_similar_products(&ProductSimilarProductsRequest::new(
                seed.product_id,
                embedding,
                request.language,
            ))
            .await?;

        Ok(GetSimilarProductsResult::Ready(products))
    }
}

impl From<ProductEmbeddingReadError> for GetSimilarProductsError {
    fn from(error: ProductEmbeddingReadError) -> Self {
        match error {
            ProductEmbeddingReadError::ProductEmbeddingQueryFailed { source } => {
                Self::ProductEmbeddingQueryFailed { source }
            }
        }
    }
}

impl From<ProductSimilarProductsReadError> for GetSimilarProductsError {
    fn from(_: ProductSimilarProductsReadError) -> Self {
        Self::SimilaritySearchUnavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{ProductEmbedding, ProductSimilarProductsReadError};
    use common::error::boxed::box_error;
    use common::product_id::ProductId;
    use common::transaction::TransactionError;
    use std::sync::{Arc, Mutex, MutexGuard};

    #[derive(Debug, Default)]
    struct FakeState {
        begin_error: bool,
        commit_error: bool,
        find_embedding_result: Option<Result<Option<ProductEmbedding>, ProductEmbeddingReadError>>,
        find_similar_products_result:
            Option<Result<Vec<ProductSummary>, ProductSimilarProductsReadError>>,
        requested_product_ids: Vec<ProductId>,
        requested_similar_products: Vec<ProductSimilarProductsRequest>,
        commit_count: usize,
    }

    type SharedState = Arc<Mutex<FakeState>>;

    #[derive(Clone)]
    struct FakeUnitOfWork {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeEmbeddingReaderFactory {
        state: SharedState,
    }

    struct FakeTx {
        state: SharedState,
    }

    struct FakeEmbeddingReader {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeSimilarProductsReader {
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

    impl ProductEmbeddingReaderFactory<FakeTx> for FakeEmbeddingReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl ProductEmbeddingReader + 'tx {
            FakeEmbeddingReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductEmbeddingReader for FakeEmbeddingReader {
        async fn find_embedding(
            &mut self,
            lookup: &ProductEmbeddingLookup,
        ) -> Result<Option<ProductEmbedding>, ProductEmbeddingReadError> {
            let product_id = match lookup {
                ProductEmbeddingLookup::ById(product_id) => *product_id,
                ProductEmbeddingLookup::BySlug { .. } => ProductId::new(),
            };
            let mut state = lock_state(&self.state);
            state.requested_product_ids.push(product_id);
            match state.find_embedding_result.take() {
                Some(result) => result,
                None => Ok(None),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductSimilarProductsReader for FakeSimilarProductsReader {
        async fn find_similar_products(
            &self,
            request: &ProductSimilarProductsRequest,
        ) -> Result<Vec<ProductSummary>, ProductSimilarProductsReadError> {
            let mut state = lock_state(&self.state);
            state.requested_similar_products.push(request.clone());
            match state.find_similar_products_result.take() {
                Some(result) => result,
                None => Ok(Vec::new()),
            }
        }
    }

    fn handler(
        state: &SharedState,
    ) -> GetSimilarProductsHandler<
        FakeUnitOfWork,
        FakeEmbeddingReaderFactory,
        FakeSimilarProductsReader,
    > {
        GetSimilarProductsHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(state),
            },
            FakeEmbeddingReaderFactory {
                state: Arc::clone(state),
            },
            FakeSimilarProductsReader {
                state: Arc::clone(state),
            },
        )
    }

    fn request() -> GetSimilarProductsRequest {
        GetSimilarProductsRequest {
            lookup: ProductEmbeddingLookup::ById(ProductId::new()),
            language: Language::En,
        }
    }

    #[tokio::test]
    async fn should_return_embedding_pending_when_product_embedding_is_missing() {
        let state = state();
        let request = request();
        lock_state(&state).find_embedding_result = Some(Ok(Some(ProductEmbedding {
            product_id: ProductId::new(),
            embedding: None,
        })));

        let result = handler(&state).execute(request.clone()).await;

        assert!(matches!(
            result,
            Ok(GetSimilarProductsResult::EmbeddingPending)
        ));
        assert_eq!(
            vec![match request.lookup {
                ProductEmbeddingLookup::ById(product_id) => product_id,
                ProductEmbeddingLookup::BySlug { .. } => ProductId::new(),
            }],
            lock_state(&state).requested_product_ids
        );
        assert_eq!(1, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_return_not_found_when_product_id_is_missing() {
        let state = state();

        let result = handler(&state).execute(request()).await;

        assert!(matches!(result, Err(GetSimilarProductsError::NotFound)));
        assert_eq!(0, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_return_ready_products_when_embedding_is_available() {
        let state = state();
        let product_id = ProductId::new();
        lock_state(&state).find_embedding_result = Some(Ok(Some(ProductEmbedding {
            product_id,
            embedding: Some(vec![0.1_f32]),
        })));

        let result = handler(&state).execute(request()).await;

        assert!(
            matches!(result, Ok(GetSimilarProductsResult::Ready(products)) if products.is_empty())
        );
        let state = lock_state(&state);
        assert_eq!(1, state.commit_count);
        assert_eq!(1, state.requested_similar_products.len());
        assert_eq!(product_id, state.requested_similar_products[0].product_id);
        assert_eq!(vec![0.1_f32], state.requested_similar_products[0].embedding);
        assert_eq!(Language::En, state.requested_similar_products[0].language);
    }

    #[tokio::test]
    async fn should_map_similar_products_reader_failure_to_unavailable_after_commit() {
        let state = state();
        lock_state(&state).find_embedding_result = Some(Ok(Some(ProductEmbedding {
            product_id: ProductId::new(),
            embedding: Some(vec![0.1_f32]),
        })));
        lock_state(&state).find_similar_products_result = Some(Err(
            ProductSimilarProductsReadError::SimilarProductsQueryFailed {
                source: box_error(std::io::Error::other("boom")),
            },
        ));

        let result = handler(&state).execute(request()).await;

        assert!(matches!(
            result,
            Err(GetSimilarProductsError::SimilaritySearchUnavailable)
        ));
        assert_eq!(1, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_map_embedding_query_failure() {
        let state = state();
        lock_state(&state).find_embedding_result = Some(Err(
            ProductEmbeddingReadError::ProductEmbeddingQueryFailed {
                source: box_error(std::io::Error::other("boom")),
            },
        ));

        let result = handler(&state).execute(request()).await;

        assert!(matches!(
            result,
            Err(GetSimilarProductsError::ProductEmbeddingQueryFailed { .. })
        ));
        assert_eq!(0, lock_state(&state).commit_count);
    }
}
