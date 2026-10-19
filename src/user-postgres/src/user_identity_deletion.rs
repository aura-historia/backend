use application::error::{box_error, static_error};
use platform_postgres::SqlxTransaction;
use sqlx::PgConnection;
use user_core::user_id::UserId;
use user_service::ports::user_identity_deletion::{
    UserIdentityDeletionClaim, UserIdentityDeletionClaimOutcome, UserIdentityDeletionError,
    UserIdentityDeletionStore, UserIdentityDeletionStoreFactory, UserIdentityUsername,
};
use user_service::ports::{CognitoIdentity, CognitoIssuer, CognitoSubject};

#[derive(Clone, Copy, Default)]
pub struct SqlxUserIdentityDeletionStoreFactory;

struct SqlxUserIdentityDeletionStore<'tx> {
    connection: &'tx mut PgConnection,
}

impl UserIdentityDeletionStoreFactory<SqlxTransaction> for SqlxUserIdentityDeletionStoreFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl UserIdentityDeletionStore + 'tx {
        SqlxUserIdentityDeletionStore {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl UserIdentityDeletionStore for SqlxUserIdentityDeletionStore<'_> {
    async fn request_for_user(&mut self, user_id: UserId) -> Result<(), UserIdentityDeletionError> {
        sqlx::query(
            "INSERT INTO user_identity_deletions (user_id, issuer, subject)
             SELECT user_id, issuer, subject FROM user_cognito_identities WHERE user_id = $1
             ON CONFLICT (user_id) DO NOTHING",
        )
        .bind(user_id.as_uuid())
        .execute(&mut *self.connection)
        .await
        .map_err(storage_error)?;
        sqlx::query(
            "INSERT INTO user_identity_deletion_requests (user_id)
             SELECT user_id FROM user_identity_deletions WHERE user_id = $1
             ON CONFLICT DO NOTHING",
        )
        .bind(user_id.as_uuid())
        .execute(&mut *self.connection)
        .await
        .map_err(storage_error)?;
        Ok(())
    }

    async fn claim(
        &mut self,
        user_id: UserId,
    ) -> Result<UserIdentityDeletionClaimOutcome, UserIdentityDeletionError> {
        let row: Option<(String, String, Option<String>, i64)> = sqlx::query_as(
            "UPDATE user_identity_deletions SET version = version + 1,
             lease_expires_at = clock_timestamp() + interval '5 minutes'
             WHERE user_id = $1 AND completed_at IS NULL
             AND (lease_expires_at IS NULL OR lease_expires_at <= now())
             RETURNING issuer, subject, username, version",
        )
        .bind(user_id.as_uuid())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(storage_error)?;
        if let Some((issuer, subject, username, version)) = row {
            let bound: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM user_cognito_identities WHERE issuer = $1 AND subject = $2)",
            )
            .bind(&issuer)
            .bind(&subject)
            .fetch_one(&mut *self.connection)
            .await
            .map_err(storage_error)?;
            if bound {
                return Err(invalid("identity deletion target is still bound"));
            }
            return Ok(UserIdentityDeletionClaimOutcome::Claimed(
                UserIdentityDeletionClaim {
                    user_id,
                    identity: CognitoIdentity {
                        issuer: CognitoIssuer::try_from(issuer).map_err(invalid_state)?,
                        subject: CognitoSubject::try_from(subject).map_err(invalid_state)?,
                    },
                    username: username.map(UserIdentityUsername::try_from).transpose()?,
                    version: u64::try_from(version).map_err(invalid_state)?,
                },
            ));
        }
        let completed: Option<bool> = sqlx::query_scalar(
            "SELECT completed_at IS NOT NULL FROM user_identity_deletions WHERE user_id = $1",
        )
        .bind(user_id.as_uuid())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(storage_error)?;
        match completed {
            Some(true) => Ok(UserIdentityDeletionClaimOutcome::Completed),
            Some(false) => Ok(UserIdentityDeletionClaimOutcome::Deferred),
            None => Err(invalid("identity deletion request is missing")),
        }
    }

    async fn save_username(
        &mut self,
        claim: &UserIdentityDeletionClaim,
        username: &UserIdentityUsername,
    ) -> Result<(), UserIdentityDeletionError> {
        let result = sqlx::query(
            "UPDATE user_identity_deletions SET username = $3
             WHERE user_id = $1 AND version = $2 AND completed_at IS NULL
             AND (username IS NULL OR username = $3)",
        )
        .bind(claim.user_id.as_uuid())
        .bind(i64::try_from(claim.version).map_err(invalid_state)?)
        .bind(username.as_str())
        .execute(&mut *self.connection)
        .await
        .map_err(storage_error)?;
        if result.rows_affected() != 1 {
            return Err(invalid("stale identity deletion claim"));
        }
        Ok(())
    }

    async fn complete(
        &mut self,
        claim: &UserIdentityDeletionClaim,
    ) -> Result<(), UserIdentityDeletionError> {
        let result = sqlx::query(
            "UPDATE user_identity_deletions SET completed_at = clock_timestamp(), lease_expires_at = NULL
             WHERE user_id = $1 AND version = $2 AND completed_at IS NULL AND username IS NOT NULL",
        )
        .bind(claim.user_id.as_uuid())
        .bind(i64::try_from(claim.version).map_err(invalid_state)?)
        .execute(&mut *self.connection)
        .await
        .map_err(storage_error)?;
        if result.rows_affected() != 1 {
            return Err(invalid("stale identity deletion claim"));
        }
        Ok(())
    }
}

fn storage_error(source: sqlx::Error) -> UserIdentityDeletionError {
    UserIdentityDeletionError(box_error(source))
}

fn invalid_state(
    source: impl std::error::Error + Send + Sync + 'static,
) -> UserIdentityDeletionError {
    UserIdentityDeletionError(box_error(source))
}

fn invalid(message: &'static str) -> UserIdentityDeletionError {
    UserIdentityDeletionError(static_error(message))
}
