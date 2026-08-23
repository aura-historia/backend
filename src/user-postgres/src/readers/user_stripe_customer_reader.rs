use crate::mapping::{UserRow, user_columns};
use application::error::box_error;
use platform_postgres::SqlxTransaction;
use user_service::ports::{
    UserStripeCustomerReadError, UserStripeCustomerReader, UserStripeCustomerReaderFactory,
};
use user_service::use_cases::queries::find_user_by_stripe_customer_id::{
    FindUserByStripeCustomerIdRequest, UserStripeLookupView,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxUserStripeCustomerReaderFactory;

struct SqlxUserStripeCustomerReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

impl SqlxUserStripeCustomerReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl UserStripeCustomerReaderFactory<SqlxTransaction> for SqlxUserStripeCustomerReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl UserStripeCustomerReader + 'tx {
        SqlxUserStripeCustomerReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl UserStripeCustomerReader for SqlxUserStripeCustomerReader<'_> {
    async fn find_by_stripe_customer_id(
        &mut self,
        request: &FindUserByStripeCustomerIdRequest,
    ) -> Result<Option<UserStripeLookupView>, UserStripeCustomerReadError> {
        let sql = format!(
            "SELECT {} FROM users WHERE stripe_customer_id = $1",
            user_columns()
        );
        let row = sqlx::query_as::<_, UserRow>(&sql)
            .bind(request.stripe_customer_id.as_ref())
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(
                |source| UserStripeCustomerReadError::TemporarilyUnavailable {
                    source: box_error(source),
                },
            )?;

        row.map(UserStripeLookupView::try_from)
            .transpose()
            .map_err(|source| UserStripeCustomerReadError::InvalidReadModel {
                source: box_error(source),
            })
    }
}
