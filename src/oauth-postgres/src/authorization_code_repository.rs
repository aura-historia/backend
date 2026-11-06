use crate::mapping::{AUTHORIZATION_CODE_COLUMNS, scope_values};
use crate::rows::AuthorizationCodeRow;
use application::error::box_error;
use oauth_core::authorization_code::{AuthorizationCode, OAuthAuthorizationCode};
use oauth_core::client_identifier::OAuthClientIdentifier;
use oauth_service::ports::{
    AuthorizationCodeRepository, AuthorizationCodeRepositoryFactory, OAuthCodeRepositoryError,
};
use platform_postgres::SqlxTransaction;
use sqlx::{PgConnection, Postgres, QueryBuilder};
use user_core::user_id::UserId;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxAuthorizationCodeRepositoryFactory;

struct SqlxAuthorizationCodeRepository<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxAuthorizationCodeRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl AuthorizationCodeRepositoryFactory<SqlxTransaction>
    for SqlxAuthorizationCodeRepositoryFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl AuthorizationCodeRepository + 'tx {
        SqlxAuthorizationCodeRepository {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl AuthorizationCodeRepository for SqlxAuthorizationCodeRepository<'_> {
    async fn insert(&mut self, code: AuthorizationCode) -> Result<(), OAuthCodeRepositoryError> {
        let code_value: String = code.code().into();
        let (client_id, metadata_document_client_id) = match code.client_id() {
            OAuthClientIdentifier::Registered(client_id) => (Some(client_id.into_uuid()), None),
            OAuthClientIdentifier::MetadataDocument(client_id) => (None, Some(client_id.as_str())),
        };
        sqlx::query(
            "INSERT INTO oauth_authorization_codes (\
                authorization_code, client_id, metadata_document_client_id, user_id, redirect_uri, \
                scopes, code_challenge, code_challenge_method, expires_at, resource, \
                issues_refresh_token\
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(code_value)
        .bind(client_id)
        .bind(metadata_document_client_id)
        .bind(code.user_id().into_uuid())
        .bind(code.redirect_uri().as_str())
        .bind(scope_values(code.scopes()))
        .bind(code.code_challenge().as_ref())
        .bind("S256")
        .bind(code.expires())
        .bind(code.resource().map(|resource| resource.as_str()))
        .bind(code.issues_refresh_token())
        .execute(&mut *self.connection)
        .await
        .map(|_| ())
        .map_err(write_code_error)
    }

    async fn consume_by_code(
        &mut self,
        code: &OAuthAuthorizationCode,
    ) -> Result<Option<AuthorizationCode>, OAuthCodeRepositoryError> {
        let code_value: String = (*code).into();
        let mut query = QueryBuilder::<Postgres>::new(
            "DELETE FROM oauth_authorization_codes WHERE authorization_code = $1 RETURNING ",
        );
        query.push(AUTHORIZATION_CODE_COLUMNS);
        let row = query
            .build_query_as::<AuthorizationCodeRow>()
            .bind(code_value)
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(temporary_code_error)?;

        row.map(TryInto::try_into)
            .transpose()
            .map_err(invalid_code_state)
    }

    async fn delete_for_user_and_client(
        &mut self,
        user_id: UserId,
        client_id: &OAuthClientIdentifier,
    ) -> Result<u64, OAuthCodeRepositoryError> {
        let query = match client_id {
            OAuthClientIdentifier::Registered(client_id) => sqlx::query(
                "DELETE FROM oauth_authorization_codes WHERE user_id = $1 AND client_id = $2",
            )
            .bind(user_id.into_uuid())
            .bind(client_id.into_uuid()),
            OAuthClientIdentifier::MetadataDocument(client_id) => sqlx::query(
                "DELETE FROM oauth_authorization_codes \
                 WHERE user_id = $1 AND metadata_document_client_id = $2",
            )
            .bind(user_id.into_uuid())
            .bind(client_id.as_str()),
        };
        query
            .execute(&mut *self.connection)
            .await
            .map(|result| result.rows_affected())
            .map_err(write_code_error)
    }
}

fn write_code_error(source: sqlx::Error) -> OAuthCodeRepositoryError {
    if let sqlx::Error::Database(database_error) = &source
        && database_error.is_unique_violation()
    {
        return OAuthCodeRepositoryError::Conflict {
            source: box_error(source),
        };
    }

    temporary_code_error(source)
}

fn temporary_code_error(source: sqlx::Error) -> OAuthCodeRepositoryError {
    OAuthCodeRepositoryError::TemporarilyUnavailable {
        source: box_error(source),
    }
}

fn invalid_code_state(
    source: impl std::error::Error + Send + Sync + 'static,
) -> OAuthCodeRepositoryError {
    OAuthCodeRepositoryError::InvalidPersistedState {
        source: box_error(source),
    }
}
