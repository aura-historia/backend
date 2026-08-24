use crate::access_token_mapping::{AccessTokenDetailsRow, access_token_id_uuid};
use application::error::box_error;
use sqlx::PgPool;
use user_core::access_token::AccessTokenId;
use user_core::user_id::UserId;
use user_service::ports::{
    AccessTokenDetails, AccessTokenDetailsReadError, AccessTokenDetailsReader,
};

#[derive(Debug, Clone)]
pub struct SqlxAccessTokenDetailsReader {
    pool: PgPool,
}

impl SqlxAccessTokenDetailsReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl AccessTokenDetailsReader for SqlxAccessTokenDetailsReader {
    async fn find_by_id(
        &self,
        user_id: UserId,
        access_token_id: AccessTokenId,
    ) -> Result<Option<AccessTokenDetails>, AccessTokenDetailsReadError> {
        let access_token_id = access_token_id_uuid(access_token_id).map_err(|source| {
            AccessTokenDetailsReadError::InvalidReadModel {
                source: box_error(source),
            }
        })?;
        let row = sqlx::query_as::<_, AccessTokenDetailsRow>(
            "SELECT access_token_id, user_id, name, scopes, origin, oauth_client_id, expires_at FROM access_tokens WHERE user_id = $1 AND access_token_id = $2",
        )
        .bind(uuid::Uuid::from(user_id))
        .bind(access_token_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|source| AccessTokenDetailsReadError::TemporarilyUnavailable {
            source: box_error(source),
        })?;

        row.map(AccessTokenDetails::try_from)
            .transpose()
            .map_err(|source| AccessTokenDetailsReadError::InvalidReadModel {
                source: box_error(source),
            })
    }
}
