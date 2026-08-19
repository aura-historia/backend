use crate::mapping::{APPLICATION_COLUMNS, PartnerShopApplicationRow};
use common::error::boxed::box_error;
use common::user_id::UserId;
use platform_postgres::SqlxTransaction;
use shop_partner_service::ports::{
    PartnerShopApplicationReader, PartnerShopApplicationReaderFactory,
    PartnerShopApplicationRepositoryError, PartnerShopApplicationView,
};
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartnerShopApplicationReaderFactory;

struct SqlxPartnerShopApplicationReader<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxPartnerShopApplicationReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl PartnerShopApplicationReaderFactory<SqlxTransaction>
    for SqlxPartnerShopApplicationReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PartnerShopApplicationReader + 'tx {
        SqlxPartnerShopApplicationReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl PartnerShopApplicationReader for SqlxPartnerShopApplicationReader<'_> {
    async fn list_all(
        &mut self,
    ) -> Result<Vec<PartnerShopApplicationView>, PartnerShopApplicationRepositoryError> {
        let sql = format!(
            "SELECT {} FROM partner_shop_applications ORDER BY created DESC",
            APPLICATION_COLUMNS
        );
        fetch_many(self.connection, &sql).await
    }

    async fn list_by_user(
        &mut self,
        user_id: UserId,
    ) -> Result<Vec<PartnerShopApplicationView>, PartnerShopApplicationRepositoryError> {
        let sql = format!(
            "SELECT {} FROM partner_shop_applications WHERE applicant_user_id = $1 ORDER BY created DESC",
            APPLICATION_COLUMNS
        );
        let rows = sqlx::query_as::<_, PartnerShopApplicationRow>(&sql)
            .bind(uuid::Uuid::from(user_id))
            .fetch_all(&mut *self.connection)
            .await
            .map_err(PartnerShopApplicationLookupSqlxError)?;
        map_rows(rows)
    }
}

async fn fetch_many(
    connection: &mut PgConnection,
    sql: &str,
) -> Result<Vec<PartnerShopApplicationView>, PartnerShopApplicationRepositoryError> {
    let rows = sqlx::query_as::<_, PartnerShopApplicationRow>(sql)
        .fetch_all(connection)
        .await
        .map_err(PartnerShopApplicationLookupSqlxError)?;
    map_rows(rows)
}

fn map_rows(
    rows: Vec<PartnerShopApplicationRow>,
) -> Result<Vec<PartnerShopApplicationView>, PartnerShopApplicationRepositoryError> {
    rows.into_iter()
        .map(PartnerShopApplicationView::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(
            |source| PartnerShopApplicationRepositoryError::InvalidPersistedState {
                source: box_error(source),
            },
        )
}

struct PartnerShopApplicationLookupSqlxError(sqlx::Error);

impl From<PartnerShopApplicationLookupSqlxError> for PartnerShopApplicationRepositoryError {
    fn from(error: PartnerShopApplicationLookupSqlxError) -> Self {
        Self::TemporarilyUnavailable {
            source: box_error(error.0),
        }
    }
}
