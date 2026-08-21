use super::SqlxSearchFilterReader;
use crate::mapping::{MATCH_COLUMNS, MatchRow, user_search_filter_uuid};
use application::pagination::{Cursor, CursoredResult};
use domain_primitives::sort::SortOrder;
use search_filter_service::ports::{
    SearchFilterMatchCursor, SearchFilterMatchListItem, SearchFilterMatchListQuery,
    SearchFilterMatchReadError, SearchFilterMatchReader,
};
use sqlx::PgPool;

#[async_trait::async_trait]
impl SearchFilterMatchReader for SqlxSearchFilterReader {
    async fn list_for_owned_filter(
        &self,
        query: &SearchFilterMatchListQuery,
    ) -> Result<
        Option<CursoredResult<SearchFilterMatchListItem, SearchFilterMatchCursor>>,
        SearchFilterMatchReadError,
    > {
        let filter_id = user_search_filter_uuid(query.search_filter_id)
            .map_err(|_| SearchFilterMatchReadError::ReadFailed)?;
        let owned: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM search_filters WHERE user_search_filter_id=$1 AND user_id=$2)",
        )
        .bind(filter_id)
        .bind(uuid::Uuid::from(query.user_id))
        .fetch_one(&self.pool)
        .await
        .map_err(|_| SearchFilterMatchReadError::ReadFailed)?;
        if !owned {
            return Ok(None);
        }

        let cursor = query.cursor.unwrap_or_default();
        let mut rows = match_rows(
            &self.pool,
            filter_id,
            cursor.search_after,
            cursor.size.saturating_add(1),
            query.order,
        )
        .await?;
        let has_more = rows.len()
            > usize::try_from(cursor.size).map_err(|_| SearchFilterMatchReadError::ReadFailed)?;
        if has_more {
            rows.pop();
        }
        let items = rows
            .into_iter()
            .map(|row| SearchFilterMatchListItem {
                product_id: row.product_id.into(),
                created: row.created,
            })
            .collect::<Vec<_>>();
        let search_after = has_more
            .then(|| {
                items.last().map(|item| SearchFilterMatchCursor {
                    created: item.created,
                    product_id: item.product_id,
                })
            })
            .flatten();

        Ok(Some(CursoredResult {
            items,
            cursor: Cursor {
                size: cursor.size,
                search_after,
            },
            total: None,
        }))
    }
}

async fn match_rows(
    pool: &PgPool,
    filter_id: uuid::Uuid,
    after: Option<SearchFilterMatchCursor>,
    size: u64,
    order: SortOrder,
) -> Result<Vec<MatchRow>, SearchFilterMatchReadError> {
    let sql = match (order, after.is_some()) {
        (SortOrder::Asc, true) => format!(
            "SELECT {MATCH_COLUMNS} FROM search_filter_matches WHERE user_search_filter_id=$1 AND (created>$2 OR (created=$2 AND product_id>$3)) ORDER BY created ASC, product_id ASC LIMIT $4"
        ),
        (SortOrder::Asc, false) => format!(
            "SELECT {MATCH_COLUMNS} FROM search_filter_matches WHERE user_search_filter_id=$1 ORDER BY created ASC, product_id ASC LIMIT $2"
        ),
        (SortOrder::Desc, true) => format!(
            "SELECT {MATCH_COLUMNS} FROM search_filter_matches WHERE user_search_filter_id=$1 AND (created<$2 OR (created=$2 AND product_id>$3)) ORDER BY created DESC, product_id ASC LIMIT $4"
        ),
        (SortOrder::Desc, false) => format!(
            "SELECT {MATCH_COLUMNS} FROM search_filter_matches WHERE user_search_filter_id=$1 ORDER BY created DESC, product_id ASC LIMIT $2"
        ),
    };
    let mut query = sqlx::query_as::<_, MatchRow>(&sql).bind(filter_id);
    if let Some(after) = after {
        query = query
            .bind(after.created)
            .bind(uuid::Uuid::from(after.product_id));
    }
    query
        .bind(i64::try_from(size).map_err(|_| SearchFilterMatchReadError::ReadFailed)?)
        .fetch_all(pool)
        .await
        .map_err(|_| SearchFilterMatchReadError::ReadFailed)
}
