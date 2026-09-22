use admin_overview_service::{
    ports::{AdminOverviewReadError, AdminOverviewReader, AdminOverviewReaderFactory},
    use_cases::get_admin_overview::{
        AdminOverview, AdminOverviewActiveListingAvailabilityCounts,
        AdminOverviewListingSourceMethodAssignmentCounts, AdminOverviewListingSources,
        AdminOverviewPartnershipApplicationStateCounts, AdminOverviewPartnershipApplications,
        AdminOverviewProductListingLifecycleCounts, AdminOverviewProductListings,
        AdminOverviewUserRoleCounts, AdminOverviewUserTierCounts, AdminOverviewUsers,
    },
};
use application::error::box_error;
use platform_postgres::SqlxTransaction;

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
    parties_total: i64,
    listing_sources_total: i64,
    listing_sources_without_ingestion_method: i64,
    listing_source_web_crawl_assignments: i64,
    listing_source_shopify_assignments: i64,
    listing_source_woocommerce_assignments: i64,
    listing_source_partner_api_assignments: i64,
    partnerships_total: i64,
    product_listings_total: i64,
    product_listings_active: i64,
    product_listings_withdrawn: i64,
    active_available: i64,
    active_in_stock: i64,
    active_limited_availability: i64,
    active_back_order: i64,
    active_made_to_order: i64,
    active_pre_order: i64,
    active_pre_sale: i64,
    active_unavailable: i64,
    active_reserved: i64,
    active_out_of_stock: i64,
    active_sold_out: i64,
    active_without_availability: i64,
}

#[derive(Debug, thiserror::Error)]
enum AdminOverviewRowMappingError {
    #[error("invalid persisted {field} count")]
    Count {
        field: &'static str,
        #[source]
        source: std::num::TryFromIntError,
    },
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
),
product_listing_counts AS (
    SELECT
        COUNT(*)::bigint AS product_listings_total,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE')::bigint AS product_listings_active,
        COUNT(*) FILTER (WHERE lifecycle = 'WITHDRAWN')::bigint AS product_listings_withdrawn,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability = 'AVAILABLE')::bigint AS active_available,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability = 'IN_STOCK')::bigint AS active_in_stock,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability = 'LIMITED_AVAILABILITY')::bigint AS active_limited_availability,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability = 'BACK_ORDER')::bigint AS active_back_order,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability = 'MADE_TO_ORDER')::bigint AS active_made_to_order,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability = 'PRE_ORDER')::bigint AS active_pre_order,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability = 'PRE_SALE')::bigint AS active_pre_sale,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability = 'UNAVAILABLE')::bigint AS active_unavailable,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability = 'RESERVED')::bigint AS active_reserved,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability = 'OUT_OF_STOCK')::bigint AS active_out_of_stock,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability = 'SOLD_OUT')::bigint AS active_sold_out,
        COUNT(*) FILTER (WHERE lifecycle = 'ACTIVE' AND availability IS NULL)::bigint AS active_without_availability
    FROM product_listings
)
SELECT *
FROM user_counts
CROSS JOIN partnership_application_counts
CROSS JOIN party_counts
CROSS JOIN listing_source_counts
CROSS JOIN listing_source_method_counts
CROSS JOIN partnership_counts
CROSS JOIN product_listing_counts
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

        AdminOverview::try_from(row).map_err(|source| AdminOverviewReadError::InvalidReadModel {
            source: box_error(source),
        })
    }
}

impl TryFrom<AdminOverviewRow> for AdminOverview {
    type Error = AdminOverviewRowMappingError;

    fn try_from(row: AdminOverviewRow) -> Result<Self, Self::Error> {
        let count = |field, value| {
            u64::try_from(value)
                .map_err(|source| AdminOverviewRowMappingError::Count { field, source })
        };

        Ok(Self {
            users: AdminOverviewUsers {
                total: count("users_total", row.users_total)?,
                by_tier: AdminOverviewUserTierCounts {
                    free: count("users_free", row.users_free)?,
                    pro: count("users_pro", row.users_pro)?,
                    ultimate: count("users_ultimate", row.users_ultimate)?,
                },
                by_role: AdminOverviewUserRoleCounts {
                    user: count("users_user", row.users_user)?,
                    admin: count("users_admin", row.users_admin)?,
                },
            },
            partnership_applications: AdminOverviewPartnershipApplications {
                total: count(
                    "partnership_applications_total",
                    row.partnership_applications_total,
                )?,
                by_state: AdminOverviewPartnershipApplicationStateCounts {
                    submitted: count(
                        "partnership_applications_submitted",
                        row.partnership_applications_submitted,
                    )?,
                    in_review: count(
                        "partnership_applications_in_review",
                        row.partnership_applications_in_review,
                    )?,
                    approved: count(
                        "partnership_applications_approved",
                        row.partnership_applications_approved,
                    )?,
                    rejected: count(
                        "partnership_applications_rejected",
                        row.partnership_applications_rejected,
                    )?,
                    withdrawn: count(
                        "partnership_applications_withdrawn",
                        row.partnership_applications_withdrawn,
                    )?,
                },
            },
            parties_total: count("parties_total", row.parties_total)?,
            listing_sources: AdminOverviewListingSources {
                total: count("listing_sources_total", row.listing_sources_total)?,
                without_ingestion_method: count(
                    "listing_sources_without_ingestion_method",
                    row.listing_sources_without_ingestion_method,
                )?,
                method_assignments: AdminOverviewListingSourceMethodAssignmentCounts {
                    web_crawl: count(
                        "listing_source_web_crawl_assignments",
                        row.listing_source_web_crawl_assignments,
                    )?,
                    shopify: count(
                        "listing_source_shopify_assignments",
                        row.listing_source_shopify_assignments,
                    )?,
                    woocommerce: count(
                        "listing_source_woocommerce_assignments",
                        row.listing_source_woocommerce_assignments,
                    )?,
                    partner_api: count(
                        "listing_source_partner_api_assignments",
                        row.listing_source_partner_api_assignments,
                    )?,
                },
            },
            partnerships_total: count("partnerships_total", row.partnerships_total)?,
            product_listings: AdminOverviewProductListings {
                total: count("product_listings_total", row.product_listings_total)?,
                by_lifecycle: AdminOverviewProductListingLifecycleCounts {
                    active: count("product_listings_active", row.product_listings_active)?,
                    withdrawn: count("product_listings_withdrawn", row.product_listings_withdrawn)?,
                },
                active_availability: AdminOverviewActiveListingAvailabilityCounts {
                    available: count("active_available", row.active_available)?,
                    in_stock: count("active_in_stock", row.active_in_stock)?,
                    limited_availability: count(
                        "active_limited_availability",
                        row.active_limited_availability,
                    )?,
                    back_order: count("active_back_order", row.active_back_order)?,
                    made_to_order: count("active_made_to_order", row.active_made_to_order)?,
                    pre_order: count("active_pre_order", row.active_pre_order)?,
                    pre_sale: count("active_pre_sale", row.active_pre_sale)?,
                    unavailable: count("active_unavailable", row.active_unavailable)?,
                    reserved: count("active_reserved", row.active_reserved)?,
                    out_of_stock: count("active_out_of_stock", row.active_out_of_stock)?,
                    sold_out: count("active_sold_out", row.active_sold_out)?,
                },
                active_without_availability: count(
                    "active_without_availability",
                    row.active_without_availability,
                )?,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn should_reject_negative_persisted_count() {
        let result = AdminOverview::try_from(AdminOverviewRow {
            users_total: -1,
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
            parties_total: 0,
            listing_sources_total: 0,
            listing_sources_without_ingestion_method: 0,
            listing_source_web_crawl_assignments: 0,
            listing_source_shopify_assignments: 0,
            listing_source_woocommerce_assignments: 0,
            listing_source_partner_api_assignments: 0,
            partnerships_total: 0,
            product_listings_total: 0,
            product_listings_active: 0,
            product_listings_withdrawn: 0,
            active_available: 0,
            active_in_stock: 0,
            active_limited_availability: 0,
            active_back_order: 0,
            active_made_to_order: 0,
            active_pre_order: 0,
            active_pre_sale: 0,
            active_unavailable: 0,
            active_reserved: 0,
            active_out_of_stock: 0,
            active_sold_out: 0,
            active_without_availability: 0,
        });

        assert!(matches!(
            result,
            Err(AdminOverviewRowMappingError::Count { .. })
        ));
    }
}
