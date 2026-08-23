use application::error::BoxError;
use search_filter_core::SearchFilterProductMatch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFilterMatchPersistOutcome {
    Inserted,
    AlreadyExists,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SearchFilterMatchBatchPersistResult {
    pub inserted: usize,
    pub already_exists: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchFilterMatchWriteError {
    #[error("search filter match persistence failed")]
    WriteFailed {
        #[source]
        source: BoxError,
    },
    #[error("persisted search filter match state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait SearchFilterMatchWriter: Send {
    async fn insert_if_absent(
        &mut self,
        product_match: &SearchFilterProductMatch,
    ) -> Result<SearchFilterMatchPersistOutcome, SearchFilterMatchWriteError>;

    async fn insert_all_if_absent(
        &mut self,
        product_matches: &[SearchFilterProductMatch],
    ) -> Result<SearchFilterMatchBatchPersistResult, SearchFilterMatchWriteError> {
        let mut result = SearchFilterMatchBatchPersistResult::default();
        for product_match in product_matches {
            match self.insert_if_absent(product_match).await? {
                SearchFilterMatchPersistOutcome::Inserted => result.inserted += 1,
                SearchFilterMatchPersistOutcome::AlreadyExists => result.already_exists += 1,
            }
        }
        Ok(result)
    }
}

pub trait SearchFilterMatchWriterFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl SearchFilterMatchWriter + 'tx;
}
