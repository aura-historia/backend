use crate::mapping::{FILTER_COLUMNS, FilterRow, user_search_filter_uuid};
use application::error::box_error;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::ports::{
    SearchFilterIndexReadError, SearchFilterIndexReader, SearchFilterProjection,
};
use sqlx::PgPool;

#[derive(Clone)]
pub struct SqlxSearchFilterIndexReader {
    pool: PgPool,
}

impl SqlxSearchFilterIndexReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl SearchFilterIndexReader for SqlxSearchFilterIndexReader {
    async fn find_by_id(
        &self,
        search_filter_id: UserSearchFilterId,
    ) -> Result<Option<SearchFilterProjection>, SearchFilterIndexReadError> {
        let search_filter_id = user_search_filter_uuid(search_filter_id).map_err(|source| {
            SearchFilterIndexReadError::ReadFailed {
                source: box_error(source),
            }
        })?;
        let sql =
            format!("SELECT {FILTER_COLUMNS} FROM search_filters WHERE user_search_filter_id=$1");
        sqlx::query_as::<_, FilterRow>(&sql)
            .bind(search_filter_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|source| SearchFilterIndexReadError::ReadFailed {
                source: box_error(source),
            })?
            .map(FilterRow::into_projection)
            .transpose()
    }

    async fn list_after(
        &self,
        after: Option<UserSearchFilterId>,
        limit: usize,
    ) -> Result<Vec<SearchFilterProjection>, SearchFilterIndexReadError> {
        let after = after
            .map(user_search_filter_uuid)
            .transpose()
            .map_err(|source| SearchFilterIndexReadError::ReadFailed {
                source: box_error(source),
            })?;
        let limit =
            i64::try_from(limit).map_err(|source| SearchFilterIndexReadError::ReadFailed {
                source: box_error(source),
            })?;
        let sql = format!(
            "SELECT {FILTER_COLUMNS} FROM search_filters \
             WHERE ($1::uuid IS NULL OR user_search_filter_id > $1) \
             ORDER BY user_search_filter_id ASC LIMIT $2"
        );
        sqlx::query_as::<_, FilterRow>(&sql)
            .bind(after)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(|source| SearchFilterIndexReadError::ReadFailed {
                source: box_error(source),
            })?
            .into_iter()
            .map(FilterRow::into_projection)
            .collect()
    }
}
