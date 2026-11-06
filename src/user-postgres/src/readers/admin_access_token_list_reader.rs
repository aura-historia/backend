use crate::access_token_list_mapping::{ACCESS_TOKEN_LIST_ENTRIES, AccessTokenListEntryRow};
use application::error::box_error;
use application::pagination::{Cursor, CursoredResult};
use platform_postgres::SqlxTransaction;
use sqlx::{AssertSqlSafe, PgConnection};
use user_core::user_id::UserId;
use user_service::ports::{
    AccessTokenListEntry, AccessTokenListEntryId, AdminAccessTokenListReadError,
    AdminAccessTokenListReader, AdminAccessTokenListReaderFactory,
};
use user_service::use_cases::queries::list_admin_access_tokens::AccessTokenSearchCursor;

const MAX_CURSOR_SIZE: u64 = 100;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxAdminAccessTokenListReaderFactory;

struct SqlxAdminAccessTokenListReader<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxAdminAccessTokenListReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl AdminAccessTokenListReaderFactory<SqlxTransaction> for SqlxAdminAccessTokenListReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl AdminAccessTokenListReader + 'tx {
        SqlxAdminAccessTokenListReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl AdminAccessTokenListReader for SqlxAdminAccessTokenListReader<'_> {
    async fn list_for_user(
        &mut self,
        user_id: UserId,
        cursor: Cursor<AccessTokenSearchCursor>,
    ) -> Result<
        CursoredResult<AccessTokenListEntry, AccessTokenSearchCursor>,
        AdminAccessTokenListReadError,
    > {
        let size = cursor.size.clamp(1, MAX_CURSOR_SIZE);
        let size_usize =
            usize::try_from(size).map_err(|source| AdminAccessTokenListReadError::Internal {
                source: box_error(source),
            })?;
        let limit =
            i64::try_from(size + 1).map_err(|source| AdminAccessTokenListReadError::Internal {
                source: box_error(source),
            })?;

        let sql = format!(
            "SELECT * FROM ({ACCESS_TOKEN_LIST_ENTRIES}) AS entries \
             WHERE $2::timestamptz IS NULL OR (created, entry_id) > ($2, $3) \
             ORDER BY created ASC, entry_id ASC LIMIT $4"
        );
        let search_after = cursor.search_after;
        let mut rows = sqlx::query_as::<_, AccessTokenListEntryRow>(AssertSqlSafe(sql))
            .bind(user_id.into_uuid())
            .bind(search_after.map(|cursor| cursor.position))
            .bind(search_after.map(|cursor| entry_uuid(cursor.entry_id)))
            .bind(limit)
            .fetch_all(&mut *self.connection)
            .await
            .map_err(
                |source| AdminAccessTokenListReadError::TemporarilyUnavailable {
                    source: box_error(source),
                },
            )?;

        let has_more = rows.len() > size_usize;
        if has_more {
            rows.truncate(size_usize);
        }
        let items_with_position = rows
            .into_iter()
            .map(|row| {
                let created = row.created();
                AccessTokenListEntry::try_from(row).map(|entry| (entry, created))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| AdminAccessTokenListReadError::InvalidReadModel {
                source: box_error(source),
            })?;

        let search_after = if has_more {
            items_with_position
                .last()
                .map(|(entry, created)| AccessTokenSearchCursor {
                    position: *created,
                    entry_id: entry.id(),
                })
        } else {
            None
        };
        let items = items_with_position
            .into_iter()
            .map(|(entry, _)| entry)
            .collect();

        Ok(CursoredResult {
            items,
            cursor: Cursor { size, search_after },
            total: None,
        })
    }
}

fn entry_uuid(entry_id: AccessTokenListEntryId) -> uuid::Uuid {
    match entry_id {
        AccessTokenListEntryId::AccessToken(id) => id.into_uuid(),
        AccessTokenListEntryId::Connection(id) => id.into_uuid(),
    }
}
