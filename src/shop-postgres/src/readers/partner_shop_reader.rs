use common::error::boxed::box_error;
use common::postgres::SqlxTransaction;
use shop_service::ports::{PartnerShopReadError, PartnerShopReader, PartnerShopReaderFactory};
use shop_service::use_cases::queries::check_user_partner_shop::CheckUserPartnerShopRequest;
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartnerShopReaderFactory;

struct SqlxPartnerShopReader<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxPartnerShopReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl PartnerShopReaderFactory<SqlxTransaction> for SqlxPartnerShopReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PartnerShopReader + 'tx {
        SqlxPartnerShopReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl PartnerShopReader for SqlxPartnerShopReader<'_> {
    async fn is_user_partner_of_shop(
        &mut self,
        request: &CheckUserPartnerShopRequest,
    ) -> Result<bool, PartnerShopReadError> {
        sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM user_partner_shops
                WHERE user_id = $1 AND shop_id = $2
            )
            "#,
        )
        .bind(uuid::Uuid::from(request.user_id))
        .bind(uuid::Uuid::from(request.shop_id))
        .fetch_one(&mut *self.connection)
        .await
        .map_err(PartnerShopReadSqlxError)
        .map_err(Into::into)
    }
}

struct PartnerShopReadSqlxError(sqlx::Error);

impl From<PartnerShopReadSqlxError> for PartnerShopReadError {
    fn from(error: PartnerShopReadSqlxError) -> Self {
        let PartnerShopReadSqlxError(source) = error;
        Self::TemporarilyUnavailable {
            source: box_error(source),
        }
    }
}
