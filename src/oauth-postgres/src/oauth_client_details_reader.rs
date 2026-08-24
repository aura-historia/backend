use crate::mapping::{OAUTH_CLIENT_VIEW_COLUMNS, client_id_uuid};
use crate::rows::OAuthClientViewRow;
use application::error::box_error;
use credential_core::oauth_client_id::OAuthClientId;
use oauth_service::ports::{OAuthClientDetailsReader, OAuthClientReadError, OAuthClientView};
use sqlx::PgPool;

#[derive(Clone)]
pub struct SqlxOAuthClientDetailsReader {
    pool: PgPool,
}

impl SqlxOAuthClientDetailsReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl OAuthClientDetailsReader for SqlxOAuthClientDetailsReader {
    async fn find(
        &self,
        client_id: &OAuthClientId,
    ) -> Result<Option<OAuthClientView>, OAuthClientReadError> {
        let client_id = client_id_uuid(client_id).map_err(invalid_persisted_state)?;
        let query =
            format!("SELECT {OAUTH_CLIENT_VIEW_COLUMNS} FROM oauth_clients WHERE client_id = $1");
        let row = sqlx::query_as::<_, OAuthClientViewRow>(&query)
            .bind(client_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(internal_error)?;

        row.map(TryInto::try_into)
            .transpose()
            .map_err(invalid_persisted_state)
    }
}

fn internal_error(source: sqlx::Error) -> OAuthClientReadError {
    OAuthClientReadError::Internal {
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
