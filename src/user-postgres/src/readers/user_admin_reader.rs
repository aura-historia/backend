use application::error::box_error;
use platform_postgres::SqlxTransaction;
use strum::IntoEnumIterator;
use user_core::role::UserRole;
use user_core::user_id::UserId;
use user_service::ports::{
    UserAdminActorView, UserAdminReadError, UserAdminReader, UserAdminReaderFactory,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxUserAdminReaderFactory;

struct SqlxUserAdminReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

#[derive(sqlx::FromRow)]
struct UserAdminActorRow {
    user_id: uuid::Uuid,
    role: String,
}

impl SqlxUserAdminReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl UserAdminReaderFactory<SqlxTransaction> for SqlxUserAdminReaderFactory {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut SqlxTransaction) -> impl UserAdminReader + 'tx {
        SqlxUserAdminReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl UserAdminReader for SqlxUserAdminReader<'_> {
    async fn find_admin_actor(
        &mut self,
        user_id: UserId,
    ) -> Result<Option<UserAdminActorView>, UserAdminReadError> {
        let row = sqlx::query_as::<_, UserAdminActorRow>(
            "SELECT user_id, role FROM users WHERE user_id = $1",
        )
        .bind(uuid::Uuid::from(user_id))
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(|source| UserAdminReadError::TemporarilyUnavailable {
            source: box_error(source),
        })?;

        row.map(TryInto::try_into).transpose()
    }
}

impl TryFrom<UserAdminActorRow> for UserAdminActorView {
    type Error = UserAdminReadError;

    fn try_from(row: UserAdminActorRow) -> Result<Self, Self::Error> {
        let role = UserRole::iter()
            .find(|role| role.as_str() == row.role)
            .ok_or_else(|| UserAdminReadError::InvalidReadModel {
                source: box_error(InvalidAdminRole),
            })?;

        Ok(Self {
            user_id: UserId::from(row.user_id),
            role,
        })
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid persisted admin role")]
struct InvalidAdminRole;
