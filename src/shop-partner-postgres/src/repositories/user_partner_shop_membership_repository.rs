use common::error::boxed::box_error;
use common::user_id::UserId;
use platform_postgres::SqlxTransaction;
use shop_core::shop_id::ShopId;
use shop_partner_service::ports::{
    UserPartnerShopMembershipRepository, UserPartnerShopMembershipRepositoryError,
    UserPartnerShopMembershipRepositoryFactory,
};
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxUserPartnerShopMembershipRepositoryFactory;

struct SqlxUserPartnerShopMembershipRepository<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxUserPartnerShopMembershipRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl UserPartnerShopMembershipRepositoryFactory<SqlxTransaction>
    for SqlxUserPartnerShopMembershipRepositoryFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl UserPartnerShopMembershipRepository + 'tx {
        SqlxUserPartnerShopMembershipRepository {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl UserPartnerShopMembershipRepository for SqlxUserPartnerShopMembershipRepository<'_> {
    async fn grant(
        &mut self,
        user_id: UserId,
        shop_id: ShopId,
    ) -> Result<(), UserPartnerShopMembershipRepositoryError> {
        sqlx::query(
            "INSERT INTO user_partner_shops (user_id, shop_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(uuid::Uuid::from(user_id))
        .bind(uuid::Uuid::from(shop_id))
        .execute(&mut *self.connection)
        .await
        .map(|_| ())
        .map_err(|source| UserPartnerShopMembershipRepositoryError::Internal {
            source: box_error(source),
        })
    }
}
