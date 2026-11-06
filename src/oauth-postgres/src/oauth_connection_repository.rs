use crate::mapping::{OAUTH_CONNECTION_COLUMNS, scope_values};
use crate::rows::OAuthConnectionRow;
use application::error::box_error;
use credential_core::metadata_document_client_id::MetadataDocumentClientId;
use credential_core::oauth_connection_id::OAuthConnectionId;
use oauth_core::connection::OAuthConnection;
use oauth_service::ports::{
    OAuthConnectionRepository, OAuthConnectionRepositoryError, OAuthConnectionRepositoryFactory,
};
use platform_postgres::SqlxTransaction;
use sqlx::{AssertSqlSafe, PgConnection};
use user_core::user_id::UserId;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxOAuthConnectionRepositoryFactory;

struct SqlxOAuthConnectionRepository<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxOAuthConnectionRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl OAuthConnectionRepositoryFactory<SqlxTransaction> for SqlxOAuthConnectionRepositoryFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl OAuthConnectionRepository + 'tx {
        SqlxOAuthConnectionRepository {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl OAuthConnectionRepository for SqlxOAuthConnectionRepository<'_> {
    async fn find_by_user_and_client_for_update(
        &mut self,
        user_id: UserId,
        client_id: &MetadataDocumentClientId,
    ) -> Result<Option<OAuthConnection>, OAuthConnectionRepositoryError> {
        let sql = format!(
            "SELECT {OAUTH_CONNECTION_COLUMNS} FROM oauth_connections \
             WHERE user_id = $1 AND client_id = $2 FOR UPDATE"
        );
        sqlx::query_as::<_, OAuthConnectionRow>(AssertSqlSafe(sql))
            .bind(user_id.into_uuid())
            .bind(client_id.as_str())
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(temporary_error)?
            .map(OAuthConnection::try_from)
            .transpose()
            .map_err(invalid_state_error)
    }

    async fn find_by_id_for_update(
        &mut self,
        connection_id: OAuthConnectionId,
    ) -> Result<Option<OAuthConnection>, OAuthConnectionRepositoryError> {
        let sql = format!(
            "SELECT {OAUTH_CONNECTION_COLUMNS} FROM oauth_connections \
             WHERE connection_id = $1 FOR UPDATE"
        );
        sqlx::query_as::<_, OAuthConnectionRow>(AssertSqlSafe(sql))
            .bind(connection_id.into_uuid())
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(temporary_error)?
            .map(OAuthConnection::try_from)
            .transpose()
            .map_err(invalid_state_error)
    }

    async fn insert(
        &mut self,
        connection: &OAuthConnection,
    ) -> Result<(), OAuthConnectionRepositoryError> {
        sqlx::query(
            "INSERT INTO oauth_connections \
                (connection_id, user_id, client_id, client_name, scopes, created, last_used) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(connection.id().into_uuid())
        .bind(connection.user_id().into_uuid())
        .bind(connection.client_id().as_str())
        .bind(connection.client_name().as_ref())
        .bind(scope_values(connection.scopes()))
        .bind(connection.created())
        .bind(connection.last_used())
        .execute(&mut *self.connection)
        .await
        .map(|_| ())
        .map_err(write_error)
    }

    async fn update(
        &mut self,
        connection: &OAuthConnection,
    ) -> Result<(), OAuthConnectionRepositoryError> {
        let updated = sqlx::query(
            "UPDATE oauth_connections SET client_name = $3, scopes = $4, last_used = $5 \
             WHERE connection_id = $1 AND user_id = $2",
        )
        .bind(connection.id().into_uuid())
        .bind(connection.user_id().into_uuid())
        .bind(connection.client_name().as_ref())
        .bind(scope_values(connection.scopes()))
        .bind(connection.last_used())
        .execute(&mut *self.connection)
        .await
        .map_err(write_error)?;
        if updated.rows_affected() == 1 {
            Ok(())
        } else {
            // Callers update only connections they hold locked, so it cannot have vanished.
            Err(OAuthConnectionRepositoryError::Internal {
                source: application::error::static_error("locked OAuth connection is missing"),
            })
        }
    }

    async fn delete(
        &mut self,
        user_id: UserId,
        connection_id: OAuthConnectionId,
    ) -> Result<bool, OAuthConnectionRepositoryError> {
        sqlx::query("DELETE FROM oauth_connections WHERE connection_id = $1 AND user_id = $2")
            .bind(connection_id.into_uuid())
            .bind(user_id.into_uuid())
            .execute(&mut *self.connection)
            .await
            .map(|result| result.rows_affected() > 0)
            .map_err(write_error)
    }
}

pub(crate) fn temporary_error(source: sqlx::Error) -> OAuthConnectionRepositoryError {
    OAuthConnectionRepositoryError::TemporarilyUnavailable {
        source: box_error(source),
    }
}

pub(crate) fn write_error(source: sqlx::Error) -> OAuthConnectionRepositoryError {
    if let sqlx::Error::Database(database_error) = &source
        && database_error.is_unique_violation()
    {
        return OAuthConnectionRepositoryError::Conflict {
            source: box_error(source),
        };
    }
    temporary_error(source)
}

pub(crate) fn invalid_state_error(
    source: impl std::error::Error + Send + Sync + 'static,
) -> OAuthConnectionRepositoryError {
    OAuthConnectionRepositoryError::InvalidPersistedState {
        source: box_error(source),
    }
}
