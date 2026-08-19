use common::error::boxed::box_error;
use common::user_id::UserId;
use platform_postgres::SqlxTransaction;
use product_service::ports::{
    PartnerProductAuthorizationError, PartnerProductAuthorizer, PartnerProductAuthorizerFactory,
};
use shop_core::shop_id::ShopId;
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartnerProductAuthorizerFactory;

struct SqlxPartnerProductAuthorizer<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxPartnerProductAuthorizerFactory {
    pub fn new() -> Self {
        Self
    }
}

impl PartnerProductAuthorizerFactory<SqlxTransaction> for SqlxPartnerProductAuthorizerFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PartnerProductAuthorizer + 'tx {
        SqlxPartnerProductAuthorizer {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl PartnerProductAuthorizer for SqlxPartnerProductAuthorizer<'_> {
    async fn authorize(
        &mut self,
        actor_id: UserId,
        shop_id: ShopId,
    ) -> Result<(), PartnerProductAuthorizationError> {
        let decision = sqlx::query_scalar::<_, String>(
            r#"
            SELECT CASE
                WHEN NOT EXISTS (
                    SELECT 1
                    FROM shops
                    WHERE shop_id = $2
                ) THEN 'SHOP_NOT_FOUND'
                WHEN EXISTS (
                    SELECT 1
                    FROM users
                    WHERE user_id = $1
                      AND role = 'ADMIN'
                ) THEN 'ALLOWED'
                WHEN EXISTS (
                    SELECT 1
                    FROM user_partner_shops partner_shops
                    JOIN shops ON shops.shop_id = partner_shops.shop_id
                    WHERE partner_shops.user_id = $1
                      AND partner_shops.shop_id = $2
                      AND shops.partner_status = 'PARTNERED'
                ) THEN 'ALLOWED'
                ELSE 'FORBIDDEN'
            END
            "#,
        )
        .bind(uuid::Uuid::from(actor_id))
        .bind(uuid::Uuid::from(shop_id))
        .fetch_one(&mut *self.connection)
        .await
        .map_err(PartnerProductAuthorizationSqlxError)?;

        match decision.as_str() {
            "ALLOWED" => Ok(()),
            "SHOP_NOT_FOUND" => Err(PartnerProductAuthorizationError::ShopNotFound),
            "FORBIDDEN" => Err(PartnerProductAuthorizationError::Forbidden),
            _ => Err(PartnerProductAuthorizationError::Internal {
                source: box_error(InvalidPartnerProductAuthorizationDecision),
            }),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("partner product authorization query failed")]
struct PartnerProductAuthorizationSqlxError(#[source] sqlx::Error);

impl From<PartnerProductAuthorizationSqlxError> for PartnerProductAuthorizationError {
    fn from(error: PartnerProductAuthorizationSqlxError) -> Self {
        Self::TemporarilyUnavailable {
            source: box_error(error),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("partner product authorization query returned an unknown decision")]
struct InvalidPartnerProductAuthorizationDecision;
