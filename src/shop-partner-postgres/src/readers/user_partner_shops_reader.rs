use common::error::boxed::box_error;
use platform_postgres::SqlxTransaction;
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use shop_core::shop_slug_id::ShopSlugId;
use shop_partner_service::ports::{
    UserPartnerShopsReadError, UserPartnerShopsReader, UserPartnerShopsReaderFactory,
};
use shop_partner_service::use_cases::list_partner_shops::{
    ListPartnerShopsRequest, ListPartnerShopsResult, PartnerShopSummary,
};
use sqlx::FromRow;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxUserPartnerShopsReaderFactory;

struct SqlxUserPartnerShopsReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

impl SqlxUserPartnerShopsReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl UserPartnerShopsReaderFactory<SqlxTransaction> for SqlxUserPartnerShopsReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl UserPartnerShopsReader + 'tx {
        SqlxUserPartnerShopsReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl UserPartnerShopsReader for SqlxUserPartnerShopsReader<'_> {
    async fn list_partner_shops(
        &mut self,
        request: &ListPartnerShopsRequest,
    ) -> Result<ListPartnerShopsResult, UserPartnerShopsReadError> {
        let rows = sqlx::query_as::<_, PartnerShopSummaryRow>(
            r#"
            SELECT s.shop_id, s.shop_slug_id, s.name
            FROM user_partner_shops ups
            JOIN shops s ON s.shop_id = ups.shop_id
            WHERE ups.user_id = $1
            ORDER BY s.name ASC
            "#,
        )
        .bind(uuid::Uuid::from(request.user_id))
        .fetch_all(&mut *self.connection)
        .await
        .map_err(|source| UserPartnerShopsReadError::TemporarilyUnavailable {
            source: box_error(source),
        })?;

        Ok(ListPartnerShopsResult {
            user_id: request.user_id,
            items: rows.into_iter().map(PartnerShopSummary::from).collect(),
        })
    }
}

#[derive(Debug, FromRow)]
struct PartnerShopSummaryRow {
    shop_id: uuid::Uuid,
    shop_slug_id: String,
    name: String,
}

impl From<PartnerShopSummaryRow> for PartnerShopSummary {
    fn from(row: PartnerShopSummaryRow) -> Self {
        Self {
            shop_id: ShopId::from(row.shop_id),
            shop_slug_id: ShopSlugId::from(row.shop_slug_id.as_str()),
            name: ShopName::from(row.name),
        }
    }
}
