use crate::mapping::FilterRow;
use common::user_id::UserId;
use search_filter_service::ports::{SearchFilterReadError, SearchFilterReader, SearchFilterView};
use sqlx::PgPool;

#[derive(Clone)]
pub struct SqlxSearchFilterReader {
    pool: PgPool,
}

impl SqlxSearchFilterReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl SearchFilterReader for SqlxSearchFilterReader {
    async fn find_for_user(
        &self,
        user_id: UserId,
    ) -> Result<Vec<SearchFilterView>, SearchFilterReadError> {
        sqlx::query_as::<_, FilterRow>(
            "SELECT user_search_filter_id, user_id, name, notifications, state, embedding, language, currency, created, updated, last_hybrid_search_matched \
             FROM search_filters WHERE user_id = $1 ORDER BY created DESC",
        )
        .bind(uuid::Uuid::from(user_id))
        .fetch_all(&self.pool)
        .await
        .map_err(|_| SearchFilterReadError::ReadFailed)?
        .into_iter()
        .map(FilterRow::into_view)
        .collect()
    }
}
