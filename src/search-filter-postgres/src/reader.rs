use crate::mapping::{FILTER_COLUMNS, FilterRow, MATCH_COLUMNS, MatchRow, user_search_filter_uuid};
use common::pagination::cursor::{Cursor, CursoredResult};
use common::product_id::{ProductId, ProductKey};
use common::sort::SortOrder;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use search_filter_service::ports::{
    ProductIdentityReadError, ProductIdentityReader, SearchFilterMatchListQuery,
    SearchFilterMatchReadError, SearchFilterMatchReader, SearchFilterMatchView,
    SearchFilterReadError, SearchFilterReader, SearchFilterView,
};
use sqlx::PgPool;
use time::OffsetDateTime;

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
#[async_trait::async_trait]
impl ProductIdentityReader for SqlxSearchFilterReader {
    async fn find_id_by_key(
        &self,
        key: &ProductKey,
    ) -> Result<Option<ProductId>, ProductIdentityReadError> {
        sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT product_id FROM products WHERE shop_id=$1 AND shops_product_id=$2",
        )
        .bind(uuid::Uuid::from(key.shop_id))
        .bind(key.shops_product_id.as_ref())
        .fetch_optional(&self.pool)
        .await
        .map(|v| v.map(ProductId::from))
        .map_err(|_| ProductIdentityReadError::LookupFailed)
    }
}
#[async_trait::async_trait]
impl SearchFilterMatchReader for SqlxSearchFilterReader {
    async fn list_for_owned_filter(
        &self,
        query: &SearchFilterMatchListQuery,
    ) -> Result<
        Option<CursoredResult<SearchFilterMatchView, OffsetDateTime>>,
        SearchFilterMatchReadError,
    > {
        let filter_id = user_search_filter_uuid(query.search_filter_id)
            .map_err(|_| SearchFilterMatchReadError::ReadFailed)?;
        let owned:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM search_filters WHERE user_search_filter_id=$1 AND user_id=$2)").bind(filter_id).bind(uuid::Uuid::from(query.user_id)).fetch_one(&self.pool).await.map_err(|_|SearchFilterMatchReadError::ReadFailed)?;
        if !owned {
            return Ok(None);
        }
        let cursor = query.cursor.unwrap_or_default();
        let rows = match query.order {
            SortOrder::Asc => match cursor.search_after {
                Some(after) => {
                    match_rows(&self.pool, filter_id, Some(after), cursor.size, true).await?
                }
                None => match_rows(&self.pool, filter_id, None, cursor.size, true).await?,
            },
            SortOrder::Desc => match cursor.search_after {
                Some(after) => {
                    match_rows(&self.pool, filter_id, Some(after), cursor.size, false).await?
                }
                None => match_rows(&self.pool, filter_id, None, cursor.size, false).await?,
            },
        };
        let items = rows
            .into_iter()
            .map(SearchFilterMatchView::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| SearchFilterMatchReadError::InvalidPersistedState)?;
        let search_after = items.last().map(|v| v.created);
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
    after: Option<OffsetDateTime>,
    size: u64,
    asc: bool,
) -> Result<Vec<MatchRow>, SearchFilterMatchReadError> {
    let sql = match (asc, after.is_some()) {
        (true, true) => format!(
            "SELECT {MATCH_COLUMNS} FROM search_filter_matches WHERE user_search_filter_id=$1 AND created>$2 ORDER BY created ASC LIMIT $3"
        ),
        (true, false) => format!(
            "SELECT {MATCH_COLUMNS} FROM search_filter_matches WHERE user_search_filter_id=$1 ORDER BY created ASC LIMIT $2"
        ),
        (false, true) => format!(
            "SELECT {MATCH_COLUMNS} FROM search_filter_matches WHERE user_search_filter_id=$1 AND created<$2 ORDER BY created DESC LIMIT $3"
        ),
        (false, false) => format!(
            "SELECT {MATCH_COLUMNS} FROM search_filter_matches WHERE user_search_filter_id=$1 ORDER BY created DESC LIMIT $2"
        ),
    };
    let mut q = sqlx::query_as::<_, MatchRow>(&sql).bind(filter_id);
    if let Some(after) = after {
        q = q.bind(after);
    }
    q.bind(i64::try_from(size).map_err(|_| SearchFilterMatchReadError::ReadFailed)?)
        .fetch_all(pool)
        .await
        .map_err(|_| SearchFilterMatchReadError::ReadFailed)
}
