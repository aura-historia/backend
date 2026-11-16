mod product_listing_counts;

use admin_overview_service::{
    ports::{AdminOverviewReadError, AdminOverviewReader, AdminOverviewReaderFactory},
    use_cases::get_admin_overview::{
        AdminOverview, AdminOverviewContentNoticeStateCounts, AdminOverviewContentNotices,
        AdminOverviewListingSourceMethodAssignmentCounts, AdminOverviewListingSources,
        AdminOverviewPartnershipApplicationStateCounts, AdminOverviewPartnershipApplications,
        AdminOverviewProductListings, AdminOverviewUserRoleCounts, AdminOverviewUserTierCounts,
        AdminOverviewUsers,
    },
};
use application::error::box_error;
use platform_postgres::SqlxTransaction;
use product_listing_counts::read_product_listing_counts;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxAdminOverviewReaderFactory;

struct SqlxAdminOverviewReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct AdminOverviewRow {
    users_total: i64,
    users_free: i64,
    users_pro: i64,
    users_ultimate: i64,
    users_user: i64,
    users_admin: i64,
    partnership_applications_total: i64,
    partnership_applications_submitted: i64,
    partnership_applications_in_review: i64,
    partnership_applications_approved: i64,
    partnership_applications_rejected: i64,
    partnership_applications_withdrawn: i64,
    content_notices_total: i64,
    content_notices_submitted: i64,
    parties_total: i64,
    listing_sources_total: i64,
    listing_sources_without_ingestion_method: i64,
    listing_source_web_crawl_assignments: i64,
    listing_source_shopify_assignments: i64,
    listing_source_woocommerce_assignments: i64,
    listing_source_partner_api_assignments: i64,
    partnerships_total: i64,
}

#[derive(Debug, thiserror::Error)]
enum AdminOverviewRowMappingError {
    #[error("invalid persisted {field} count")]
    Count {
        field: &'static str,
        #[source]
        source: std::num::TryFromIntError,
    },
    #[error("invalid {column} planner statistics: {reason}")]
    Statistics {
        column: &'static str,
        reason: &'static str,
    },
}

fn count(field: &'static str, value: i64) -> Result<u64, AdminOverviewRowMappingError> {
    u64::try_from(value).map_err(|source| AdminOverviewRowMappingError::Count { field, source })
}

impl SqlxAdminOverviewReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl AdminOverviewReaderFactory<SqlxTransaction> for SqlxAdminOverviewReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl AdminOverviewReader + 'tx {
        SqlxAdminOverviewReader {
            connection: tx.connection(),
        }
    }
}

const ADMIN_OVERVIEW_SQL: &str = r#"
WITH
user_counts AS (
    SELECT
        COUNT(*)::bigint AS users_total,
        COUNT(*) FILTER (WHERE tier = 'FREE')::bigint AS users_free,
        COUNT(*) FILTER (WHERE tier = 'PRO')::bigint AS users_pro,
        COUNT(*) FILTER (WHERE tier = 'ULTIMATE')::bigint AS users_ultimate,
        COUNT(*) FILTER (WHERE role = 'USER')::bigint AS users_user,
        COUNT(*) FILTER (WHERE role = 'ADMIN')::bigint AS users_admin
    FROM users
),
partnership_application_counts AS (
    SELECT
        COUNT(*)::bigint AS partnership_applications_total,
        COUNT(*) FILTER (WHERE business_state = 'SUBMITTED')::bigint AS partnership_applications_submitted,
        COUNT(*) FILTER (WHERE business_state = 'IN_REVIEW')::bigint AS partnership_applications_in_review,
        COUNT(*) FILTER (WHERE business_state = 'APPROVED')::bigint AS partnership_applications_approved,
        COUNT(*) FILTER (WHERE business_state = 'REJECTED')::bigint AS partnership_applications_rejected,
        COUNT(*) FILTER (WHERE business_state = 'WITHDRAWN')::bigint AS partnership_applications_withdrawn
    FROM partnership_applications
),
content_notice_counts AS (
    SELECT
        COUNT(*)::bigint AS content_notices_total,
        COUNT(*) FILTER (WHERE business_state = 'SUBMITTED')::bigint AS content_notices_submitted
    FROM content_notices
),
party_counts AS (
    SELECT COUNT(*)::bigint AS parties_total
    FROM parties
),
listing_source_counts AS (
    SELECT
        COUNT(*)::bigint AS listing_sources_total,
        COUNT(*) FILTER (
            WHERE NOT EXISTS (
                SELECT 1
                FROM listing_source_ingestion_methods methods
                WHERE methods.listing_source_id = sources.listing_source_id
            )
        )::bigint AS listing_sources_without_ingestion_method
    FROM listing_sources sources
),
listing_source_method_counts AS (
    SELECT
        COUNT(*) FILTER (WHERE ingestion_method = 'WEB_CRAWL')::bigint AS listing_source_web_crawl_assignments,
        COUNT(*) FILTER (WHERE ingestion_method = 'SHOPIFY')::bigint AS listing_source_shopify_assignments,
        COUNT(*) FILTER (WHERE ingestion_method = 'WOOCOMMERCE')::bigint AS listing_source_woocommerce_assignments,
        COUNT(*) FILTER (WHERE ingestion_method = 'PARTNER_API')::bigint AS listing_source_partner_api_assignments
    FROM listing_source_ingestion_methods
),
partnership_counts AS (
    SELECT COUNT(*)::bigint AS partnerships_total
    FROM partnerships
)
SELECT *
FROM user_counts
CROSS JOIN partnership_application_counts
CROSS JOIN content_notice_counts
CROSS JOIN party_counts
CROSS JOIN listing_source_counts
CROSS JOIN listing_source_method_counts
CROSS JOIN partnership_counts
"#;

#[async_trait::async_trait]
impl AdminOverviewReader for SqlxAdminOverviewReader<'_> {
    async fn read_overview(&mut self) -> Result<AdminOverview, AdminOverviewReadError> {
        let row = sqlx::query_as::<_, AdminOverviewRow>(ADMIN_OVERVIEW_SQL)
            .fetch_one(&mut *self.connection)
            .await
            .map_err(|source| AdminOverviewReadError::TemporarilyUnavailable {
                source: box_error(source),
            })?;

        let product_listings = read_product_listing_counts(self.connection).await?;

        row.into_overview(product_listings).map_err(|source| {
            AdminOverviewReadError::InvalidReadModel {
                source: box_error(source),
            }
        })
    }
}

impl AdminOverviewRow {
    fn into_overview(
        self,
        product_listings: AdminOverviewProductListings,
    ) -> Result<AdminOverview, AdminOverviewRowMappingError> {
        Ok(AdminOverview {
            users: AdminOverviewUsers {
                total: count("users_total", self.users_total)?,
                by_tier: AdminOverviewUserTierCounts {
                    free: count("users_free", self.users_free)?,
                    pro: count("users_pro", self.users_pro)?,
                    ultimate: count("users_ultimate", self.users_ultimate)?,
                },
                by_role: AdminOverviewUserRoleCounts {
                    user: count("users_user", self.users_user)?,
                    admin: count("users_admin", self.users_admin)?,
                },
            },
            partnership_applications: AdminOverviewPartnershipApplications {
                total: count(
                    "partnership_applications_total",
                    self.partnership_applications_total,
                )?,
                by_state: AdminOverviewPartnershipApplicationStateCounts {
                    submitted: count(
                        "partnership_applications_submitted",
                        self.partnership_applications_submitted,
                    )?,
                    in_review: count(
                        "partnership_applications_in_review",
                        self.partnership_applications_in_review,
                    )?,
                    approved: count(
                        "partnership_applications_approved",
                        self.partnership_applications_approved,
                    )?,
                    rejected: count(
                        "partnership_applications_rejected",
                        self.partnership_applications_rejected,
                    )?,
                    withdrawn: count(
                        "partnership_applications_withdrawn",
                        self.partnership_applications_withdrawn,
                    )?,
                },
            },
            content_notices: AdminOverviewContentNotices {
                total: count("content_notices_total", self.content_notices_total)?,
                by_state: AdminOverviewContentNoticeStateCounts {
                    submitted: count("content_notices_submitted", self.content_notices_submitted)?,
                },
            },
            parties_total: count("parties_total", self.parties_total)?,
            listing_sources: AdminOverviewListingSources {
                total: count("listing_sources_total", self.listing_sources_total)?,
                without_ingestion_method: count(
                    "listing_sources_without_ingestion_method",
                    self.listing_sources_without_ingestion_method,
                )?,
                method_assignments: AdminOverviewListingSourceMethodAssignmentCounts {
                    web_crawl: count(
                        "listing_source_web_crawl_assignments",
                        self.listing_source_web_crawl_assignments,
                    )?,
                    shopify: count(
                        "listing_source_shopify_assignments",
                        self.listing_source_shopify_assignments,
                    )?,
                    woocommerce: count(
                        "listing_source_woocommerce_assignments",
                        self.listing_source_woocommerce_assignments,
                    )?,
                    partner_api: count(
                        "listing_source_partner_api_assignments",
                        self.listing_source_partner_api_assignments,
                    )?,
                },
            },
            partnerships_total: count("partnerships_total", self.partnerships_total)?,
            product_listings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zero_row() -> AdminOverviewRow {
        AdminOverviewRow {
            users_total: 0,
            users_free: 0,
            users_pro: 0,
            users_ultimate: 0,
            users_user: 0,
            users_admin: 0,
            partnership_applications_total: 0,
            partnership_applications_submitted: 0,
            partnership_applications_in_review: 0,
            partnership_applications_approved: 0,
            partnership_applications_rejected: 0,
            partnership_applications_withdrawn: 0,
            content_notices_total: 0,
            content_notices_submitted: 0,
            parties_total: 0,
            listing_sources_total: 0,
            listing_sources_without_ingestion_method: 0,
            listing_source_web_crawl_assignments: 0,
            listing_source_shopify_assignments: 0,
            listing_source_woocommerce_assignments: 0,
            listing_source_partner_api_assignments: 0,
            partnerships_total: 0,
        }
    }

    #[test]
    fn should_reject_negative_persisted_count() {
        let result = AdminOverviewRow {
            users_total: -1,
            ..zero_row()
        }
        .into_overview(AdminOverviewProductListings::default());

        assert!(matches!(
            result,
            Err(AdminOverviewRowMappingError::Count { .. })
        ));
    }

    #[test]
    fn should_reject_negative_persisted_content_notice_count() {
        let result = AdminOverviewRow {
            content_notices_submitted: -1,
            ..zero_row()
        }
        .into_overview(AdminOverviewProductListings::default());

        assert!(matches!(
            result,
            Err(AdminOverviewRowMappingError::Count {
                field: "content_notices_submitted",
                ..
            })
        ));
    }

    #[test]
    fn should_map_content_notice_counts() {
        let result = AdminOverviewRow {
            content_notices_total: 3,
            content_notices_submitted: 2,
            ..zero_row()
        }
        .into_overview(AdminOverviewProductListings::default());

        let overview = match result {
            Ok(overview) => overview,
            Err(error) => panic!("overview mapping failed: {error}"),
        };
        assert_eq!(
            AdminOverviewContentNotices {
                total: 3,
                by_state: AdminOverviewContentNoticeStateCounts { submitted: 2 },
            },
            overview.content_notices
        );
    }
}
