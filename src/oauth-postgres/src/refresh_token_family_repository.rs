use crate::mapping::scope_values;
use crate::oauth_connection_repository::{invalid_state_error, temporary_error, write_error};
use crate::rows::RefreshTokenFamilyRow;
use credential_core::oauth_connection_id::OAuthConnectionId;
use oauth_core::refresh_token::{
    HashedRawRefreshToken, PresentedRefreshToken, RefreshTokenFamily, RefreshTokenFamilyId,
};
use oauth_service::ports::{
    OAuthConnectionRepositoryError, RefreshTokenFamilyRepository,
    RefreshTokenFamilyRepositoryFactory,
};
use platform_postgres::SqlxTransaction;
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxRefreshTokenFamilyRepositoryFactory;

struct SqlxRefreshTokenFamilyRepository<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxRefreshTokenFamilyRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl RefreshTokenFamilyRepositoryFactory<SqlxTransaction>
    for SqlxRefreshTokenFamilyRepositoryFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl RefreshTokenFamilyRepository + 'tx {
        SqlxRefreshTokenFamilyRepository {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl RefreshTokenFamilyRepository for SqlxRefreshTokenFamilyRepository<'_> {
    async fn insert(
        &mut self,
        family: &RefreshTokenFamily,
    ) -> Result<(), OAuthConnectionRepositoryError> {
        sqlx::query(
            "INSERT INTO oauth_refresh_token_families \
                (family_id, connection_id, scopes, resource, created, absolute_expires_at, \
                 expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(family.id().into_uuid())
        .bind(family.connection_id().into_uuid())
        .bind(scope_values(family.scopes()))
        .bind(family.resource().as_str())
        .bind(family.created())
        .bind(family.absolute_expires())
        .bind(family.current().expires)
        .execute(&mut *self.connection)
        .await
        .map_err(write_error)?;
        self.insert_current_token(family).await
    }

    async fn find_connection_id(
        &mut self,
        token: &HashedRawRefreshToken,
    ) -> Result<Option<OAuthConnectionId>, OAuthConnectionRepositoryError> {
        sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT families.connection_id \
             FROM oauth_refresh_tokens AS tokens \
             JOIN oauth_refresh_token_families AS families USING (family_id) \
             WHERE tokens.token_short = $1 AND tokens.token_hash = $2",
        )
        .bind(token.short_token())
        .bind(token.long_token_hash())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(temporary_error)?
        .map(OAuthConnectionId::try_from)
        .transpose()
        .map_err(invalid_state_error)
    }

    async fn find_by_token_for_update(
        &mut self,
        token: &HashedRawRefreshToken,
    ) -> Result<Option<(RefreshTokenFamily, PresentedRefreshToken)>, OAuthConnectionRepositoryError>
    {
        // Lock first, then read in a new statement: under READ COMMITTED a locking join would
        // keep the token rows of its original snapshot after waiting for a concurrent rotation.
        let Some(family_id) = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT family_id FROM oauth_refresh_token_families \
             WHERE family_id = ( \
                 SELECT family_id FROM oauth_refresh_tokens \
                 WHERE token_short = $1 AND token_hash = $2 \
             ) \
             FOR UPDATE",
        )
        .bind(token.short_token())
        .bind(token.long_token_hash())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(temporary_error)?
        else {
            return Ok(None);
        };
        let row = sqlx::query_as::<_, RefreshTokenFamilyRow>(
            "SELECT families.family_id, families.connection_id, families.scopes, \
                    families.resource, families.created, families.absolute_expires_at, \
                    families.expires_at, current.token_short AS current_token_short, \
                    current.token_hash AS current_token_hash, \
                    current.issued AS current_issued, \
                    presented.replaced_at AS presented_replaced_at \
             FROM oauth_refresh_token_families AS families \
             JOIN oauth_refresh_tokens AS current \
               ON current.family_id = families.family_id AND current.replaced_at IS NULL \
             JOIN oauth_refresh_tokens AS presented \
               ON presented.family_id = families.family_id \
              AND presented.token_short = $2 AND presented.token_hash = $3 \
             WHERE families.family_id = $1",
        )
        .bind(family_id)
        .bind(token.short_token())
        .bind(token.long_token_hash())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(temporary_error)?
        .ok_or_else(|| invalid_state_error(InvalidFamilyState::MissingCurrentToken))?;
        <(RefreshTokenFamily, PresentedRefreshToken)>::try_from(row)
            .map(Some)
            .map_err(invalid_state_error)
    }

    async fn record_rotation(
        &mut self,
        family: &RefreshTokenFamily,
        replaced: &HashedRawRefreshToken,
    ) -> Result<(), OAuthConnectionRepositoryError> {
        let marked = sqlx::query(
            "UPDATE oauth_refresh_tokens SET replaced_at = $4 \
             WHERE token_short = $1 AND token_hash = $2 AND family_id = $3 \
               AND replaced_at IS NULL",
        )
        .bind(replaced.short_token())
        .bind(replaced.long_token_hash())
        .bind(family.id().into_uuid())
        .bind(family.current().issued)
        .execute(&mut *self.connection)
        .await
        .map_err(write_error)?;
        if marked.rows_affected() != 1 {
            return Err(invalid_state_error(
                InvalidFamilyState::ReplacedTokenNotCurrent,
            ));
        }
        self.insert_current_token(family).await?;
        sqlx::query("UPDATE oauth_refresh_token_families SET expires_at = $2 WHERE family_id = $1")
            .bind(family.id().into_uuid())
            .bind(family.current().expires)
            .execute(&mut *self.connection)
            .await
            .map(|_| ())
            .map_err(write_error)
    }

    async fn delete(
        &mut self,
        family_id: RefreshTokenFamilyId,
    ) -> Result<(), OAuthConnectionRepositoryError> {
        sqlx::query("DELETE FROM oauth_refresh_token_families WHERE family_id = $1")
            .bind(family_id.into_uuid())
            .execute(&mut *self.connection)
            .await
            .map(|_| ())
            .map_err(write_error)
    }
}

impl SqlxRefreshTokenFamilyRepository<'_> {
    async fn insert_current_token(
        &mut self,
        family: &RefreshTokenFamily,
    ) -> Result<(), OAuthConnectionRepositoryError> {
        let current = family.current();
        sqlx::query(
            "INSERT INTO oauth_refresh_tokens (token_short, token_hash, family_id, issued) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(current.hashed_token.short_token())
        .bind(current.hashed_token.long_token_hash())
        .bind(family.id().into_uuid())
        .bind(current.issued)
        .execute(&mut *self.connection)
        .await
        .map(|_| ())
        .map_err(write_error)
    }
}

#[derive(Debug, thiserror::Error)]
enum InvalidFamilyState {
    #[error("a locked refresh-token family has no current token")]
    MissingCurrentToken,
    #[error("the replaced refresh token was not the family's current token")]
    ReplacedTokenNotCurrent,
}
