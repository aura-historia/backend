use application::error::box_error;
use listing_source_core::ListingSourceId;
use platform_postgres::SqlxTransaction;
use product_listing_service::ports::{
    PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
    PartnerProductListingAuthorizerFactory,
};
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
        listing_source_id: ListingSourceId,
    ) -> Result<(), PartnerProductListingAuthorizationError> {
        let decision = sqlx::query_scalar::<_, String>(
            r#"
            SELECT CASE
                WHEN NOT EXISTS (
                    SELECT 1
                    FROM listing_sources
                    WHERE listing_source_id = $2
                ) THEN 'LISTING_SOURCE_NOT_FOUND'
                WHEN EXISTS (
                    SELECT 1
                    FROM users
                    WHERE user_id = $1
                      AND role = 'ADMIN'
                ) THEN 'ALLOWED'
                WHEN EXISTS (
                    SELECT 1
                    FROM listing_sources source
                    JOIN partnerships partnership
                      ON partnership.party_id = source.operator_party_id
                    JOIN partnership_members member
                      ON member.partnership_id = partnership.partnership_id
                    WHERE source.listing_source_id = $2
                      AND member.user_id = $1
                ) THEN 'ALLOWED'
                WHEN EXISTS (
                    SELECT 1
                    FROM partnership_listing_source_grants source_grant
                    WHERE source_grant.user_id = $1
                      AND source_grant.listing_source_id = $2
                ) THEN 'ALLOWED'
                ELSE 'FORBIDDEN'
            END
            "#,
        )
        .bind(uuid::Uuid::from(actor_id))
        .bind(uuid::Uuid::from(listing_source_id))
        .fetch_one(&mut *self.connection)
        .await
        .map_err(PartnerProductListingAuthorizationSqlxError)?;

        match decision.as_str() {
            "ALLOWED" => Ok(()),
            "LISTING_SOURCE_NOT_FOUND" => {
                Err(PartnerProductListingAuthorizationError::ListingSourceNotFound)
            }
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
