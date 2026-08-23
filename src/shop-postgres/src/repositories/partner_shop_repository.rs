use application::error::box_error;
use platform_postgres::SqlxTransaction;
use shop_core::shop_id::ShopId;
use shop_service::ports::{
    PartnerShopRepository, PartnerShopRepositoryError, PartnerShopRepositoryFactory,
};
use sqlx::PgConnection;
use user_core::user_id::UserId;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartnerShopRepositoryFactory;

struct SqlxPartnerShopRepository<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxPartnerShopRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl PartnerShopRepositoryFactory<SqlxTransaction> for SqlxPartnerShopRepositoryFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PartnerShopRepository + 'tx {
        SqlxPartnerShopRepository {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl PartnerShopRepository for SqlxPartnerShopRepository<'_> {
    async fn grant(
        &mut self,
        user_id: UserId,
        shop_id: ShopId,
    ) -> Result<(), PartnerShopRepositoryError> {
        sqlx::query(
            r#"
            INSERT INTO user_partner_shops (user_id, shop_id)
            VALUES ($1, $2)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(uuid::Uuid::from(user_id))
        .bind(uuid::Uuid::from(shop_id))
        .execute(&mut *self.connection)
        .await
        .map_err(PartnerShopGrantSqlxError)?;

        Ok(())
    }
}

struct PartnerShopGrantSqlxError(sqlx::Error);

impl From<PartnerShopGrantSqlxError> for PartnerShopRepositoryError {
    fn from(error: PartnerShopGrantSqlxError) -> Self {
        let PartnerShopGrantSqlxError(source) = error;
        match &source {
            sqlx::Error::Database(database_error)
                if database_error.constraint() == Some("user_partner_shops_user_id_fkey") =>
            {
                Self::UserNotFound {
                    source: box_error(source),
                }
            }
            sqlx::Error::Database(database_error)
                if database_error.constraint() == Some("user_partner_shops_shop_id_fkey") =>
            {
                Self::ShopNotFound {
                    source: box_error(source),
                }
            }
            _ => Self::Internal {
                source: box_error(source),
            },
        }
    }
}
