use crate::mapping::{
    FILTER_COLUMNS, FilterRow, format_state, product_search_to_json, user_search_filter_uuid,
};
use common::postgres::SqlxTransaction;
use common::user_search_filter_id::UserSearchFilterId;
use search_filter_core::SearchFilter;
use search_filter_service::ports::{
    PersistedSearchFilter, SearchFilterRepository, SearchFilterRepositoryError,
    SearchFilterRepositoryFactory,
};

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
#[async_trait::async_trait]
impl SearchFilterRepository for SqlxSearchFilterRepository<'_> {
    async fn find_by_id(
        &mut self,
        id: UserSearchFilterId,
    ) -> Result<Option<PersistedSearchFilter>, SearchFilterRepositoryError> {
        let id =
            user_search_filter_uuid(id).map_err(|_| SearchFilterRepositoryError::LookupFailed)?;
        let sql =
            format!("SELECT {FILTER_COLUMNS} FROM search_filters WHERE user_search_filter_id=$1");
        sqlx::query_as::<_, FilterRow>(&sql)
            .bind(id)
            .fetch_optional(self.tx.connection())
            .await
            .map_err(|_| SearchFilterRepositoryError::LookupFailed)?
            .map(FilterRow::into_persisted)
            .transpose()
    }
    async fn insert(
        &mut self,
        filter: &SearchFilter,
    ) -> Result<PersistedSearchFilter, SearchFilterRepositoryError> {
        let search = product_search_to_json(filter.search())
            .map_err(|_| SearchFilterRepositoryError::InsertFailed)?;
        let id = user_search_filter_uuid(filter.id())
            .map_err(|_| SearchFilterRepositoryError::InsertFailed)?;
        let sql = format!(
            "INSERT INTO search_filters (user_search_filter_id,user_id,name,notifications,state,search,enhanced_search_description,embedding,language,currency) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING {FILTER_COLUMNS}"
        );
        let row = sqlx::query_as::<_, FilterRow>(&sql)
            .bind(id)
            .bind(uuid::Uuid::from(filter.user_id()))
            .bind(filter.name().as_ref())
            .bind(filter.notifications())
            .bind(format_state(filter.state()))
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
            .map_err(map_insert)?;
        row.into_persisted()
    }
    async fn update(
        &mut self,
        filter: &SearchFilter,
        expected_version: i64,
    ) -> Result<PersistedSearchFilter, SearchFilterRepositoryError> {
        let search = product_search_to_json(filter.search())
            .map_err(|_| SearchFilterRepositoryError::UpdateFailed)?;
        let id = user_search_filter_uuid(filter.id())
            .map_err(|_| SearchFilterRepositoryError::UpdateFailed)?;
        let sql = format!(
            "UPDATE search_filters SET name=$2,notifications=$3,state=$4,search=$5,enhanced_search_description=$6,embedding=$7,language=$8,currency=$9,version=version+1,updated=now() WHERE user_search_filter_id=$1 AND version=$10 RETURNING {FILTER_COLUMNS}"
        );
        let row = sqlx::query_as::<_, FilterRow>(&sql)
            .bind(id)
            .bind(filter.name().as_ref())
            .bind(filter.notifications())
            .bind(format_state(filter.state()))
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
            .map_err(|_| SearchFilterRepositoryError::UpdateFailed)?
            .ok_or(SearchFilterRepositoryError::ConcurrencyConflict)?;
        row.into_persisted()
    }
    async fn delete(&mut self, id: UserSearchFilterId) -> Result<(), SearchFilterRepositoryError> {
        let id =
            user_search_filter_uuid(id).map_err(|_| SearchFilterRepositoryError::DeleteFailed)?;
        let result = sqlx::query("DELETE FROM search_filters WHERE user_search_filter_id=$1")
            .bind(id)
            .execute(self.tx.connection())
            .await
            .map_err(|_| SearchFilterRepositoryError::DeleteFailed)?;
        if result.rows_affected() == 1 {
            Ok(())
        } else {
            Err(SearchFilterRepositoryError::DeleteFailed)
        }
    }
}
fn map_insert(error: sqlx::Error) -> SearchFilterRepositoryError {
    if let sqlx::Error::Database(db) = &error
        && db.is_unique_violation()
    {
        SearchFilterRepositoryError::AlreadyExists
    } else {
        SearchFilterRepositoryError::InsertFailed
    }
}
