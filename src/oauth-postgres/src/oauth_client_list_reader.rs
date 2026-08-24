use crate::mapping::OAUTH_CLIENT_VIEW_COLUMNS;
use crate::rows::OAuthClientViewRow;
use application::error::box_error;
use oauth_service::ports::{OAuthClientListReader, OAuthClientReadError, OAuthClientView};
use sqlx::PgPool;

#[derive(Clone)]
pub struct SqlxOAuthClientListReader {
    pool: PgPool,
}

impl SqlxOAuthClientListReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl OAuthClientListReader for SqlxOAuthClientListReader {
    async fn list(&self) -> Result<Vec<OAuthClientView>, OAuthClientReadError> {
        let query = format!(
            "SELECT {OAUTH_CLIENT_VIEW_COLUMNS} FROM oauth_clients ORDER BY created ASC, client_id ASC"
        );
        let rows = sqlx::query_as::<_, OAuthClientViewRow>(&query)
            .fetch_all(&self.pool)
            .await
            .map_err(temporarily_unavailable)?;

        rows.into_iter()
            .map(|row| row.try_into().map_err(invalid_persisted_state))
            .collect()
    }
}

fn temporarily_unavailable(source: sqlx::Error) -> OAuthClientReadError {
    OAuthClientReadError::TemporarilyUnavailable {
        source: box_error(source),
    }
}

fn invalid_persisted_state(
    source: impl std::error::Error + Send + Sync + 'static,
) -> OAuthClientReadError {
    OAuthClientReadError::InvalidPersistedState {
        source: box_error(source),
    }
}
