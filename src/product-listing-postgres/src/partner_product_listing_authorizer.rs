use application::error::box_error;
use platform_postgres::SqlxTransaction;
use product_listing_service::ports::{
    PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
    PartnerProductListingAuthorizerFactory,
};
use shop_core::shop_id::ShopId;
use sqlx::PgConnection;
use user_core::user_id::UserId;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartnerProductListingAuthorizerFactory;

struct SqlxPartnerProductListingAuthorizer<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxPartnerProductListingAuthorizerFactory {
    pub fn new() -> Self {
        Self
    }
}

impl PartnerProductListingAuthorizerFactory<SqlxTransaction>
    for SqlxPartnerProductListingAuthorizerFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PartnerProductListingAuthorizer + 'tx {
        SqlxPartnerProductListingAuthorizer {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl PartnerProductListingAuthorizer for SqlxPartnerProductListingAuthorizer<'_> {
    async fn authorize(
        &mut self,
        actor_id: UserId,
        shop_id: ShopId,
    ) -> Result<(), PartnerProductListingAuthorizationError> {
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
        .map_err(PartnerProductListingAuthorizationSqlxError)?;

        match decision.as_str() {
            "ALLOWED" => Ok(()),
            "SHOP_NOT_FOUND" => Err(PartnerProductListingAuthorizationError::ShopNotFound),
            "FORBIDDEN" => Err(PartnerProductListingAuthorizationError::Forbidden),
            _ => Err(PartnerProductListingAuthorizationError::Internal {
                source: box_error(InvalidPartnerProductListingAuthorizationDecision),
            }),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("partner product authorization query failed")]
struct PartnerProductListingAuthorizationSqlxError(#[source] sqlx::Error);

impl From<PartnerProductListingAuthorizationSqlxError> for PartnerProductListingAuthorizationError {
    fn from(error: PartnerProductListingAuthorizationSqlxError) -> Self {
        Self::TemporarilyUnavailable {
            source: box_error(error),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("partner product authorization query returned an unknown decision")]
struct InvalidPartnerProductListingAuthorizationDecision;
