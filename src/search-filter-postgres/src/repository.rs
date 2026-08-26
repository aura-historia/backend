use crate::mapping::{
    FILTER_COLUMNS, FilterRow, ProductListingSearchJsonMappingError, product_search_to_json,
    user_search_filter_uuid,
};
use application::error::box_error;
use platform_postgres::SqlxTransaction;
use search_filter_core::SearchFilter;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::ports::{
    PersistedSearchFilter, SearchFilterRepository, SearchFilterRepositoryError,
    SearchFilterRepositoryFactory,
};
use sqlx::{Postgres, QueryBuilder};

#[derive(Debug, Clone, Default)]
pub struct SqlxSearchFilterRepositoryFactory;
struct SqlxSearchFilterRepository<'tx> {
    tx: &'tx mut SqlxTransaction,
}
impl SearchFilterRepositoryFactory<SqlxTransaction> for SqlxSearchFilterRepositoryFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl SearchFilterRepository + 'tx {
        SqlxSearchFilterRepository { tx }
    }
}

#[derive(Debug)]
enum SearchFilterRepositoryAdapterError {
    LookupIdentifier(uuid::Error),
    LookupSqlx(sqlx::Error),
    InsertSearchSerialization(ProductListingSearchJsonMappingError),
    InsertIdentifier(uuid::Error),
    InsertSqlx(sqlx::Error),
    UpdateSearchSerialization(ProductListingSearchJsonMappingError),
    UpdateIdentifier(uuid::Error),
    UpdateSqlx(sqlx::Error),
    DeleteIdentifier(uuid::Error),
    DeleteSqlx(sqlx::Error),
    DeleteNoRowsAffected,
}

impl std::fmt::Display for SearchFilterRepositoryAdapterError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LookupIdentifier(_) => {
                formatter.write_str("search filter lookup identifier conversion failed")
            }
            Self::LookupSqlx(_) => formatter.write_str("search filter lookup SQL query failed"),
            Self::InsertSearchSerialization(_) => {
                formatter.write_str("search filter insert search serialization failed")
            }
            Self::InsertIdentifier(_) => {
                formatter.write_str("search filter insert identifier conversion failed")
            }
            Self::InsertSqlx(_) => formatter.write_str("search filter insert SQL query failed"),
            Self::UpdateSearchSerialization(_) => {
                formatter.write_str("search filter update search serialization failed")
            }
            Self::UpdateIdentifier(_) => {
                formatter.write_str("search filter update identifier conversion failed")
            }
            Self::UpdateSqlx(_) => formatter.write_str("search filter update SQL query failed"),
            Self::DeleteIdentifier(_) => {
                formatter.write_str("search filter delete identifier conversion failed")
            }
            Self::DeleteSqlx(_) => formatter.write_str("search filter delete SQL query failed"),
            Self::DeleteNoRowsAffected => {
                formatter.write_str("search filter delete did not affect exactly one row")
            }
        }
    }
}

impl std::error::Error for SearchFilterRepositoryAdapterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::LookupIdentifier(source)
            | Self::InsertIdentifier(source)
            | Self::UpdateIdentifier(source)
            | Self::DeleteIdentifier(source) => Some(source),
            Self::LookupSqlx(source)
            | Self::InsertSqlx(source)
            | Self::UpdateSqlx(source)
            | Self::DeleteSqlx(source) => Some(source),
            Self::InsertSearchSerialization(source) | Self::UpdateSearchSerialization(source) => {
                Some(source)
            }
            Self::DeleteNoRowsAffected => None,
        }
    }
}

impl From<SearchFilterRepositoryAdapterError> for SearchFilterRepositoryError {
    fn from(source: SearchFilterRepositoryAdapterError) -> Self {
        match source {
            SearchFilterRepositoryAdapterError::LookupIdentifier(_)
            | SearchFilterRepositoryAdapterError::LookupSqlx(_) => Self::LookupFailed {
                source: box_error(source),
            },
            SearchFilterRepositoryAdapterError::InsertSearchSerialization(_)
            | SearchFilterRepositoryAdapterError::InsertIdentifier(_)
            | SearchFilterRepositoryAdapterError::InsertSqlx(_) => Self::InsertFailed {
                source: box_error(source),
            },
            SearchFilterRepositoryAdapterError::UpdateSearchSerialization(_)
            | SearchFilterRepositoryAdapterError::UpdateIdentifier(_)
            | SearchFilterRepositoryAdapterError::UpdateSqlx(_) => Self::UpdateFailed {
                source: box_error(source),
            },
            SearchFilterRepositoryAdapterError::DeleteIdentifier(_)
            | SearchFilterRepositoryAdapterError::DeleteSqlx(_)
            | SearchFilterRepositoryAdapterError::DeleteNoRowsAffected => Self::DeleteFailed {
                source: box_error(source),
            },
        }
    }
}
#[async_trait::async_trait]
impl SearchFilterRepository for SqlxSearchFilterRepository<'_> {
    async fn find_by_id(
        &mut self,
        id: UserSearchFilterId,
    ) -> Result<Option<PersistedSearchFilter>, SearchFilterRepositoryError> {
        let id = user_search_filter_uuid(id)
            .map_err(SearchFilterRepositoryAdapterError::LookupIdentifier)?;
        let mut query = QueryBuilder::<Postgres>::new("SELECT ");
        query
            .push(FILTER_COLUMNS)
            .push(" FROM search_filters WHERE user_search_filter_id=$1");
        query
            .build_query_as::<FilterRow>()
            .bind(id)
            .fetch_optional(self.tx.connection())
            .await
            .map_err(SearchFilterRepositoryAdapterError::LookupSqlx)?
            .map(FilterRow::into_persisted)
            .transpose()
    }
    async fn insert(
        &mut self,
        filter: &SearchFilter,
    ) -> Result<PersistedSearchFilter, SearchFilterRepositoryError> {
        let search = product_search_to_json(filter.search())
            .map_err(SearchFilterRepositoryAdapterError::InsertSearchSerialization)?;
        let id = user_search_filter_uuid(filter.id())
            .map_err(SearchFilterRepositoryAdapterError::InsertIdentifier)?;
        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO search_filters (user_search_filter_id,user_id,name,notifications,state,search,enhanced_search_description,embedding,language,currency) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING ",
        );
        query.push(FILTER_COLUMNS);
        let row = query
            .build_query_as::<FilterRow>()
            .bind(id)
            .bind(uuid::Uuid::from(filter.user_id()))
            .bind(filter.name().as_ref())
            .bind(filter.notifications())
            .bind(filter.state().as_str())
            .bind(search)
            .bind(
                filter
                    .search()
                    .enhanced_search_description
                    .as_ref()
                    .map(AsRef::as_ref),
            )
            .bind(filter.embedding())
            .bind(filter.search().language.as_str())
            .bind(filter.search().currency.as_str())
            .fetch_one(self.tx.connection())
            .await
            .map_err(|source| {
                if let sqlx::Error::Database(database_error) = &source
                    && database_error.is_unique_violation()
                {
                    SearchFilterRepositoryError::AlreadyExists
                } else {
                    SearchFilterRepositoryAdapterError::InsertSqlx(source).into()
                }
            })?;
        row.into_persisted()
    }
    async fn update(
        &mut self,
        filter: &SearchFilter,
        expected_version: i64,
    ) -> Result<PersistedSearchFilter, SearchFilterRepositoryError> {
        let search = product_search_to_json(filter.search())
            .map_err(SearchFilterRepositoryAdapterError::UpdateSearchSerialization)?;
        let id = user_search_filter_uuid(filter.id())
            .map_err(SearchFilterRepositoryAdapterError::UpdateIdentifier)?;
        let mut query = QueryBuilder::<Postgres>::new(
            "UPDATE search_filters SET name=$2,notifications=$3,state=$4,search=$5,enhanced_search_description=$6,embedding=$7,language=$8,currency=$9,version=version+1,updated=now() WHERE user_search_filter_id=$1 AND version=$10 RETURNING ",
        );
        query.push(FILTER_COLUMNS);
        let row = query
            .build_query_as::<FilterRow>()
            .bind(id)
            .bind(filter.name().as_ref())
            .bind(filter.notifications())
            .bind(filter.state().as_str())
            .bind(search)
            .bind(
                filter
                    .search()
                    .enhanced_search_description
                    .as_ref()
                    .map(AsRef::as_ref),
            )
            .bind(filter.embedding())
            .bind(filter.search().language.as_str())
            .bind(filter.search().currency.as_str())
            .bind(expected_version)
            .fetch_optional(self.tx.connection())
            .await
            .map_err(SearchFilterRepositoryAdapterError::UpdateSqlx)?
            .ok_or(SearchFilterRepositoryError::ConcurrencyConflict)?;
        row.into_persisted()
    }
    async fn delete(&mut self, id: UserSearchFilterId) -> Result<(), SearchFilterRepositoryError> {
        let id = user_search_filter_uuid(id)
            .map_err(SearchFilterRepositoryAdapterError::DeleteIdentifier)?;
        let result = sqlx::query("DELETE FROM search_filters WHERE user_search_filter_id=$1")
            .bind(id)
            .execute(self.tx.connection())
            .await
            .map_err(SearchFilterRepositoryAdapterError::DeleteSqlx)?;
        if result.rows_affected() == 1 {
            Ok(())
        } else {
            Err(SearchFilterRepositoryAdapterError::DeleteNoRowsAffected.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_preserve_lookup_sqlx_source() {
        let error: SearchFilterRepositoryError =
            SearchFilterRepositoryAdapterError::LookupSqlx(sqlx::Error::RowNotFound).into();

        let SearchFilterRepositoryError::LookupFailed { source } = error else {
            panic!("expected search-filter lookup failure");
        };
        assert!(
            source
                .downcast_ref::<SearchFilterRepositoryAdapterError>()
                .is_some()
        );
        assert!(source.source().is_some());
    }

    #[test]
    fn should_map_missing_delete_row_to_sourced_failure() {
        let error: SearchFilterRepositoryError =
            SearchFilterRepositoryAdapterError::DeleteNoRowsAffected.into();

        let SearchFilterRepositoryError::DeleteFailed { source } = error else {
            panic!("expected search-filter delete failure");
        };
        assert!(
            source
                .downcast_ref::<SearchFilterRepositoryAdapterError>()
                .is_some()
        );
    }
}
