use crate::mapping::{FilterRow, format_state, user_search_filter_uuid};
use common::postgres::SqlxTransaction;
use common::user_search_filter_id::UserSearchFilterId;
use search_filter_core::SearchFilter;
use search_filter_service::ports::{
    SearchFilterRepository, SearchFilterRepositoryError, SearchFilterRepositoryFactory,
};
use time::OffsetDateTime;

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
    ) -> Result<Option<SearchFilter>, SearchFilterRepositoryError> {
        let row = sqlx::query_as::<_, FilterRow>(
            "SELECT user_search_filter_id, user_id, name, notifications, state, embedding, language, currency, created, updated, last_hybrid_search_matched \
             FROM search_filters WHERE user_search_filter_id = $1",
        )
        .bind(user_search_filter_uuid(id))
        .fetch_optional(self.tx.connection())
        .await
        .map_err(|_| SearchFilterRepositoryError::LookupFailed)?;
        row.map(FilterRow::into_domain).transpose()
    }

    async fn insert(
        &mut self,
        filter: &SearchFilter,
    ) -> Result<SearchFilter, SearchFilterRepositoryError> {
        let now = OffsetDateTime::now_utc();
        sqlx::query(
            "INSERT INTO search_filters \
             (user_search_filter_id, user_id, name, notifications, state, search, enhanced_search_description, embedding, language, currency, created, updated, last_hybrid_search_matched) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$11,$12)",
        )
        .bind(user_search_filter_uuid(filter.id()))
        .bind(uuid::Uuid::from(filter.user_id()))
        .bind(filter.name().as_ref())
        .bind(filter.notifications())
        .bind(format_state(filter.state()))
        .bind(serde_json::json!({}))
        .bind(filter.search().enhanced_search_description.as_ref().map(|v| v.as_ref()))
        .bind(filter.embedding())
        .bind(filter.search().language.as_str())
        .bind(filter.search().currency.as_str())
        .bind(now)
        .bind(time::OffsetDateTime::UNIX_EPOCH)
        .execute(self.tx.connection())
        .await
        .map_err(map_filter_insert_error)?;
        Ok(filter.clone())
    }

    async fn update(
        &mut self,
        filter: &SearchFilter,
    ) -> Result<SearchFilter, SearchFilterRepositoryError> {
        sqlx::query(
            "UPDATE search_filters SET name=$2, notifications=$3, state=$4, search=$5, enhanced_search_description=$6, embedding=$7, language=$8, currency=$9, updated=$10 \
             WHERE user_search_filter_id=$1",
        )
        .bind(user_search_filter_uuid(filter.id()))
        .bind(filter.name().as_ref())
        .bind(filter.notifications())
        .bind(format_state(filter.state()))
        .bind(serde_json::json!({}))
        .bind(filter.search().enhanced_search_description.as_ref().map(|v| v.as_ref()))
        .bind(filter.embedding())
        .bind(filter.search().language.as_str())
        .bind(filter.search().currency.as_str())
        .bind(OffsetDateTime::now_utc())
        .execute(self.tx.connection())
        .await
        .map_err(|_| SearchFilterRepositoryError::UpdateFailed)?;
        Ok(filter.clone())
    }

    async fn delete(&mut self, id: UserSearchFilterId) -> Result<(), SearchFilterRepositoryError> {
        sqlx::query("DELETE FROM search_filters WHERE user_search_filter_id = $1")
            .bind(user_search_filter_uuid(id))
            .execute(self.tx.connection())
            .await
            .map_err(|_| SearchFilterRepositoryError::DeleteFailed)?;
        Ok(())
    }
}

fn map_filter_insert_error(error: sqlx::Error) -> SearchFilterRepositoryError {
    if let sqlx::Error::Database(db) = &error
        && db.is_unique_violation()
    {
        return SearchFilterRepositoryError::AlreadyExists;
    }
    SearchFilterRepositoryError::InsertFailed
}
