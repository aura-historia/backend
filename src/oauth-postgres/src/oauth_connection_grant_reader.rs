use crate::mapping::OAuthRowMappingError;
use application::error::box_error;
use credential_core::metadata_document_client_id::MetadataDocumentClientId;
use credential_core::scope::Scope;
use oauth_service::ports::{OAuthClientReadError, OAuthConnectionGrantReader};
use sqlx::PgPool;
use std::collections::HashSet;
use user_core::user_id::UserId;

#[derive(Debug, Clone)]
pub struct SqlxOAuthConnectionGrantReader {
    pool: PgPool,
}

impl SqlxOAuthConnectionGrantReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl OAuthConnectionGrantReader for SqlxOAuthConnectionGrantReader {
    async fn granted_scopes(
        &self,
        user_id: UserId,
        client_id: &MetadataDocumentClientId,
    ) -> Result<HashSet<Scope>, OAuthClientReadError> {
        let scopes = sqlx::query_scalar::<_, Vec<String>>(
            "SELECT scopes FROM oauth_connections WHERE user_id = $1 AND client_id = $2",
        )
        .bind(user_id.into_uuid())
        .bind(client_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|source| OAuthClientReadError::TemporarilyUnavailable {
            source: box_error(source),
        })?
        .unwrap_or_default();
        crate::mapping::parse_scopes(scopes).map_err(|source: OAuthRowMappingError| {
            OAuthClientReadError::InvalidPersistedState {
                source: box_error(source),
            }
        })
    }
}
