use crate::access_token_list_mapping::{ACCESS_TOKEN_LIST_ENTRIES, AccessTokenListEntryRow};
use application::error::box_error;
use sqlx::{AssertSqlSafe, PgPool};
use user_core::user_id::UserId;
use user_service::ports::{AccessTokenListEntry, AccessTokenListReadError, AccessTokenListReader};

#[derive(Debug, Clone)]
pub struct SqlxAccessTokenListReader {
    pool: PgPool,
}

impl SqlxAccessTokenListReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl AccessTokenListReader for SqlxAccessTokenListReader {
    async fn list_for_user(
        &self,
        user_id: UserId,
    ) -> Result<Vec<AccessTokenListEntry>, AccessTokenListReadError> {
        let sql = format!(
            "SELECT * FROM ({ACCESS_TOKEN_LIST_ENTRIES}) AS entries \
             ORDER BY created ASC, entry_id ASC"
        );
        let rows = sqlx::query_as::<_, AccessTokenListEntryRow>(AssertSqlSafe(sql))
            .bind(user_id.into_uuid())
            .fetch_all(&self.pool)
            .await
            .map_err(|source| AccessTokenListReadError::TemporarilyUnavailable {
                source: box_error(source),
            })?;

        rows.into_iter()
            .map(AccessTokenListEntry::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| AccessTokenListReadError::InvalidReadModel {
                source: box_error(source),
            })
    }
}
