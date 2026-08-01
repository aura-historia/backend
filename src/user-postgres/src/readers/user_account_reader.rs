use crate::mapping::{UserRow, user_columns};
use common::error::boxed::box_error;
use common::postgres::SqlxTransaction;
use user_service::ports::{UserAccountReadError, UserAccountReader, UserAccountReaderFactory};
use user_service::use_cases::queries::get_user::{GetUserRequest, UserDetailsView};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxUserAccountReaderFactory;

struct SqlxUserAccountReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

impl SqlxUserAccountReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl UserAccountReaderFactory<SqlxTransaction> for SqlxUserAccountReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl UserAccountReader + 'tx {
        SqlxUserAccountReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl UserAccountReader for SqlxUserAccountReader<'_> {
    async fn find_account(
        &mut self,
        request: &GetUserRequest,
    ) -> Result<Option<UserDetailsView>, UserAccountReadError> {
        find_user_details(self.connection, request).await
    }
}

pub(crate) async fn find_user_details(
    connection: &mut sqlx::PgConnection,
    request: &GetUserRequest,
) -> Result<Option<UserDetailsView>, UserAccountReadError> {
    let sql = match request {
        GetUserRequest::ById(_) | GetUserRequest::AdminById(_) => {
            format!("SELECT {} FROM users WHERE user_id = $1", user_columns())
        }
        GetUserRequest::ByEmail(_) => {
            format!("SELECT {} FROM users WHERE email = $1", user_columns())
        }
    };

    let mut query = sqlx::query_as::<_, UserRow>(&sql);
    query = match request {
        GetUserRequest::ById(user_id) | GetUserRequest::AdminById(user_id) => {
            query.bind(uuid::Uuid::from(*user_id))
        }
        GetUserRequest::ByEmail(email) => query.bind::<&str>(email.as_ref()),
    };

    let row = query.fetch_optional(connection).await.map_err(|source| {
        UserAccountReadError::TemporarilyUnavailable {
            source: box_error(source),
        }
    })?;

    row.map(UserDetailsView::try_from)
        .transpose()
        .map_err(|source| UserAccountReadError::InvalidReadModel {
            source: box_error(source),
        })
}
