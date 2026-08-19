use crate::mapping::ShopSummaryRow;
use common::error::boxed::box_error;
use common::user_id::UserId;
use platform_postgres::SqlxTransaction;
use shop_service::ports::{PartnerShopReadError, PartnerShopReader, PartnerShopReaderFactory};
use shop_service::use_cases::queries::check_user_partner_shop::CheckUserPartnerShopRequest;
use shop_service::use_cases::queries::search_shops::ShopSummary;
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

    async fn list_summaries_for_user(
        &mut self,
        user_id: UserId,
    ) -> Result<Vec<ShopSummary>, PartnerShopReadError> {
        let rows = sqlx::query_as::<_, ShopSummaryRow>(
            r#"
            SELECT
                s.shop_id,
                s.shop_slug_id,
                s.name,
                s.shop_type,
                s.partner_status,
                s.shop_domains,
                s.image,
                s.created,
                s.updated
            FROM user_partner_shops ups
            JOIN shops s ON s.shop_id = ups.shop_id
            WHERE ups.user_id = $1
            ORDER BY ups.created ASC, s.shop_id ASC
            "#,
        )
        .bind(uuid::Uuid::from(user_id))
        .fetch_all(&mut *self.connection)
        .await
        .map_err(PartnerShopReadSqlxError)?;

        rows.into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| PartnerShopReadError::InvalidReadModel {
                source: box_error(source),
            })
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
