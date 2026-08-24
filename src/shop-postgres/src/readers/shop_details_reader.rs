use crate::mapping::{ShopRow, shop_columns};
use application::error::box_error;
use platform_postgres::SqlxTransaction;
use shop_service::ports::{ShopDetailsReadError, ShopDetailsReader, ShopDetailsReaderFactory};
use shop_service::use_cases::queries::get_shop::{GetShopRequest, ShopDetailsView};
use sqlx::{PgConnection, Postgres, QueryBuilder};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxShopDetailsReaderFactory;

struct SqlxShopDetailsReader<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxShopDetailsReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ShopDetailsReaderFactory<SqlxTransaction> for SqlxShopDetailsReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ShopDetailsReader + 'tx {
        SqlxShopDetailsReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ShopDetailsReader for SqlxShopDetailsReader<'_> {
    async fn find_details(
        &mut self,
        request: &GetShopRequest,
    ) -> Result<Option<ShopDetailsView>, ShopDetailsReadError> {
        let row = match request {
            GetShopRequest::ById(shop_id) => {
                let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
                builder
                    .push(shop_columns())
                    .push(" FROM shops WHERE shop_id = ")
                    .push_bind(uuid::Uuid::from(*shop_id))
                    .push(" AND lifecycle = 'PUBLISHED'");
                builder
                    .build_query_as::<ShopRow>()
                    .fetch_optional(&mut *self.connection)
                    .await
            }
            GetShopRequest::BySlug(slug_id) => {
                let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
                builder
                    .push(shop_columns())
                    .push(" FROM shops WHERE shop_slug_id = ")
                    .push_bind(slug_id.as_ref())
                    .push(" AND lifecycle = 'PUBLISHED'");
                builder
                    .build_query_as::<ShopRow>()
                    .fetch_optional(&mut *self.connection)
                    .await
            }
            GetShopRequest::ByShopifyDomain(domain) => {
                let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
                builder
                    .push(shop_columns())
                    .push(" FROM shops WHERE shopify_domain = ")
                    .push_bind(domain.as_str())
                    .push(" AND lifecycle = 'PUBLISHED'");
                builder
                    .build_query_as::<ShopRow>()
                    .fetch_optional(&mut *self.connection)
                    .await
            }
        }
        .map_err(ShopDetailsSqlxError)?;

        row.map(ShopDetailsView::try_from)
            .transpose()
            .map_err(|source| ShopDetailsReadError::InvalidReadModel {
                source: box_error(source),
            })
    }
}

struct ShopDetailsSqlxError(sqlx::Error);

impl From<ShopDetailsSqlxError> for ShopDetailsReadError {
    fn from(error: ShopDetailsSqlxError) -> Self {
        let ShopDetailsSqlxError(source) = error;
        Self::TemporarilyUnavailable {
            source: box_error(source),
        }
    }
}
