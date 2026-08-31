use application::error::BoxError;

use search_filter_core::SearchFilter;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct PersistedSearchFilter {
    pub filter: SearchFilter,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
    pub version: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchFilterRepositoryError {
    #[error("search filter already exists")]
    AlreadyExists,
    #[error("search filter lookup failed")]
    LookupFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter insert failed")]
    InsertFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter update failed")]
    UpdateFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter update conflicted with a concurrent write")]
    ConcurrencyConflict,
    #[error("search filter delete failed")]
    DeleteFailed {
        #[source]
        source: BoxError,
    },
    #[error("persisted search filter state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait SearchFilterRepository: Send {
    async fn find_by_id(
        &mut self,
        id: UserSearchFilterId,
    ) -> Result<Option<PersistedSearchFilter>, SearchFilterRepositoryError>;
    async fn insert(
        &mut self,
        filter: &SearchFilter,
    ) -> Result<PersistedSearchFilter, SearchFilterRepositoryError>;
    async fn update(
        &mut self,
        filter: &SearchFilter,
        expected_version: i64,
    ) -> Result<PersistedSearchFilter, SearchFilterRepositoryError>;
    async fn delete(&mut self, id: UserSearchFilterId) -> Result<(), SearchFilterRepositoryError>;
}

pub trait SearchFilterRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl SearchFilterRepository + 'tx;
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::static_error;
    use std::error::Error;

    #[test]
    fn should_preserve_repository_lookup_failure_source() {
        let error = SearchFilterRepositoryError::LookupFailed {
            source: static_error("database connection lost"),
        };

        assert_eq!(
            Some("database connection lost"),
            error.source().map(ToString::to_string).as_deref()
        );
    }

    #[test]
    fn should_preserve_invalid_persisted_state_source() {
        let error = SearchFilterRepositoryError::InvalidPersistedState {
            source: static_error("persisted state is malformed"),
        };

        assert_eq!(
            Some("persisted state is malformed"),
            error.source().map(ToString::to_string).as_deref()
        );
    }
}
