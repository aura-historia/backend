use crate::readers::user_account_reader::find_user_details;
use common::postgres::SqlxTransaction;
use user_service::ports::{UserAdminReadError, UserAdminReader, UserAdminReaderFactory};
use user_service::use_cases::queries::get_user::{GetUserRequest, UserDetailsView};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxUserAdminReaderFactory;

struct SqlxUserAdminReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
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
    async fn find_admin_view(
        &mut self,
        request: &GetUserRequest,
    ) -> Result<Option<UserDetailsView>, UserAdminReadError> {
        find_user_details(self.connection, request)
            .await
            .map_err(|error| match error {
                user_service::ports::UserAccountReadError::TemporarilyUnavailable { source } => {
                    UserAdminReadError::TemporarilyUnavailable { source }
                }
                user_service::ports::UserAccountReadError::InvalidReadModel { source } => {
                    UserAdminReadError::InvalidReadModel { source }
                }
                user_service::ports::UserAccountReadError::Internal { source } => {
                    UserAdminReadError::Internal { source }
                }
            })
    }
}
