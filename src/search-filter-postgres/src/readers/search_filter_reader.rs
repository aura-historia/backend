use crate::mapping::{FILTER_COLUMNS, FilterRow, user_search_filter_uuid};
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::ports::{SearchFilterReadError, SearchFilterReader, SearchFilterView};
use sqlx::PgPool;
use user_core::user_id::UserId;

#[derive(Clone)]
pub struct SqlxSearchFilterReader {
    pub(super) pool: PgPool,
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
        let sql = format!(
            "SELECT {FILTER_COLUMNS} FROM search_filters WHERE user_id=$1 ORDER BY created DESC"
        );
        sqlx::query_as::<_, FilterRow>(&sql)
            .bind(uuid::Uuid::from(user_id))
            .fetch_all(&self.pool)
            .await
            .map_err(|_| SearchFilterReadError::ReadFailed)?
            .into_iter()
            .map(FilterRow::into_view)
            .collect()
    }

    async fn find_for_user_by_id(
        &self,
        user_id: UserId,
        id: UserSearchFilterId,
    ) -> Result<Option<SearchFilterView>, SearchFilterReadError> {
        let id = user_search_filter_uuid(id).map_err(|_| SearchFilterReadError::ReadFailed)?;
        let sql = format!(
            "SELECT {FILTER_COLUMNS} FROM search_filters WHERE user_id=$1 AND user_search_filter_id=$2"
        );
        sqlx::query_as::<_, FilterRow>(&sql)
            .bind(uuid::Uuid::from(user_id))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| SearchFilterReadError::ReadFailed)?
            .map(FilterRow::into_view)
            .transpose()
    }
}
