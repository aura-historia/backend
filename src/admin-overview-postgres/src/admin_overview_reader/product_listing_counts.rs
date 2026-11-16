//! ProductListing counts for the admin overview.
//!
//! Counting every listing reads the whole table, so from [`EXACT_COUNT_LISTING_LIMIT`] estimated
//! listings on the counts come from PostgreSQL planner statistics instead: the live tuple counter
//! for the total and the most-common-value frequencies that (auto)analyze samples for the
//! breakdown. The estimate reads catalog rows only and takes no lock on `product_listings`.
//! Below the limit, or before the table was ever vacuumed or analyzed, listings are counted
//! exactly. A large table without column statistics (for example right after a major-version
//! upgrade) is reported as temporarily unavailable instead of being scanned.

use super::{AdminOverviewRowMappingError, count};
use admin_overview_service::{
    ports::AdminOverviewReadError,
    use_cases::get_admin_overview::{
        AdminOverviewActiveListingAvailabilityCounts, AdminOverviewProductListingLifecycleCounts,
        AdminOverviewProductListings,
    },
};
use application::error::box_error;

/// Tables with fewer estimated listings are counted exactly.
const EXACT_COUNT_LISTING_LIMIT: u64 = 100_000;

const LIFECYCLE_VALUES: [&str; 2] = ["ACTIVE", "WITHDRAWN"];

/// Order matches [`availability_counts`].
const AVAILABILITY_VALUES: [&str; 11] = [
    "AVAILABLE",
    "IN_STOCK",
    "LIMITED_AVAILABILITY",
    "BACK_ORDER",
    "MADE_TO_ORDER",
    "PRE_ORDER",
    "PRE_SALE",
    "UNAVAILABLE",
    "RESERVED",
    "OUT_OF_STOCK",
    "SOLD_OUT",
];

/// Float4 frequencies may sum slightly above one.
const FREQUENCY_SUM_TOLERANCE: f64 = 1e-4;

const PRODUCT_LISTING_COUNTS_SQL: &str = r#"
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
"#;

// `n_live_tup` follows every committed insert and delete, but a crash, failover or statistics
// reset zeroes it until the next vacuum or analyze; `reltuples` from the last vacuum or analyze
// covers that gap and is -1 until the table was first processed. The frequencies are NULL until
// the column was first analyzed, and also for a column whose sample was all NULL.
const PRODUCT_LISTING_STATISTICS_SQL: &str = r#"
SELECT
    CASE
        WHEN COALESCE(
            activity.last_vacuum, activity.last_autovacuum,
            activity.last_analyze, activity.last_autoanalyze
        ) IS NOT NULL THEN activity.n_live_tup
        WHEN listing_table.reltuples >= 0 THEN listing_table.reltuples::bigint
    END AS estimated_total,
    lifecycle.attname IS NOT NULL AS lifecycle_analyzed,
    lifecycle.most_common_vals::text::text[] AS lifecycle_values,
    lifecycle.most_common_freqs::float8[] AS lifecycle_frequencies,
    availability.attname IS NOT NULL AS availability_analyzed,
    availability.most_common_vals::text::text[] AS availability_values,
    availability.most_common_freqs::float8[] AS availability_frequencies
FROM pg_class listing_table
JOIN pg_namespace listing_schema ON listing_schema.oid = listing_table.relnamespace
LEFT JOIN pg_stat_user_tables activity ON activity.relid = listing_table.oid
LEFT JOIN pg_stats lifecycle
  ON lifecycle.schemaname = listing_schema.nspname
 AND lifecycle.tablename = listing_table.relname
 AND lifecycle.attname = 'lifecycle'
 AND NOT lifecycle.inherited
LEFT JOIN pg_stats availability
  ON availability.schemaname = listing_schema.nspname
 AND availability.tablename = listing_table.relname
 AND availability.attname = 'availability'
 AND NOT availability.inherited
WHERE listing_table.oid = 'product_listings'::regclass
"#;

#[derive(Debug, sqlx::FromRow)]
pub(super) struct ProductListingCountsRow {
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

#[derive(Debug, sqlx::FromRow)]
struct ProductListingStatisticsRow {
    estimated_total: Option<i64>,
    lifecycle_analyzed: bool,
    lifecycle_values: Option<Vec<String>>,
    lifecycle_frequencies: Option<Vec<f64>>,
    availability_analyzed: bool,
    availability_values: Option<Vec<String>>,
    availability_frequencies: Option<Vec<f64>>,
}

#[derive(Debug, thiserror::Error)]
#[error(
    "product_listings has no planner statistics for its {column} column; run ANALYZE product_listings"
)]
struct MissingColumnStatistics {
    column: &'static str,
}

#[derive(Debug)]
enum CountMethod {
    Exact,
    Estimate(ProductListingStatistics),
}

/// Validated planner statistics for `product_listings`.
#[derive(Debug)]
struct ProductListingStatistics {
    estimated_total: u64,
    active_frequency: f64,
    /// Indexed like [`AVAILABILITY_VALUES`].
    availability_frequencies: [f64; 11],
}

pub(super) async fn read_product_listing_counts(
    connection: &mut sqlx::PgConnection,
) -> Result<AdminOverviewProductListings, AdminOverviewReadError> {
    let statistics =
        sqlx::query_as::<_, ProductListingStatisticsRow>(PRODUCT_LISTING_STATISTICS_SQL)
            .fetch_one(&mut *connection)
            .await
            .map_err(|source| AdminOverviewReadError::TemporarilyUnavailable {
                source: box_error(source),
            })?;
    if let CountMethod::Estimate(statistics) = statistics.count_method()? {
        return Ok(statistics.estimate());
    }

    let counts = sqlx::query_as::<_, ProductListingCountsRow>(PRODUCT_LISTING_COUNTS_SQL)
        .fetch_one(&mut *connection)
        .await
        .map_err(|source| AdminOverviewReadError::TemporarilyUnavailable {
            source: box_error(source),
        })?;
    AdminOverviewProductListings::try_from(counts).map_err(invalid_read_model)
}

fn invalid_read_model(source: AdminOverviewRowMappingError) -> AdminOverviewReadError {
    AdminOverviewReadError::InvalidReadModel {
        source: box_error(source),
    }
}

impl ProductListingStatisticsRow {
    /// Statistics are validated only when they are used, so they cannot fail an exact count.
    fn count_method(self) -> Result<CountMethod, AdminOverviewReadError> {
        let Some(estimated_total) = self.estimated_total else {
            return Ok(CountMethod::Exact);
        };
        let estimated_total = count("estimated_product_listings_total", estimated_total)
            .map_err(invalid_read_model)?;
        if estimated_total < EXACT_COUNT_LISTING_LIMIT {
            return Ok(CountMethod::Exact);
        }
        for (column, analyzed) in [
            ("lifecycle", self.lifecycle_analyzed),
            ("availability", self.availability_analyzed),
        ] {
            if !analyzed {
                return Err(AdminOverviewReadError::TemporarilyUnavailable {
                    source: box_error(MissingColumnStatistics { column }),
                });
            }
        }
        let [active_frequency, _withdrawn_frequency] = frequencies(
            "lifecycle",
            &LIFECYCLE_VALUES,
            self.lifecycle_values,
            self.lifecycle_frequencies,
        )
        .map_err(invalid_read_model)?;
        let availability_frequencies = frequencies(
            "availability",
            &AVAILABILITY_VALUES,
            self.availability_values,
            self.availability_frequencies,
        )
        .map_err(invalid_read_model)?;
        Ok(CountMethod::Estimate(ProductListingStatistics {
            estimated_total,
            active_frequency,
            availability_frequencies,
        }))
    }
}

impl ProductListingStatistics {
    /// Withdrawn listings never carry an availability (`product_listings` CHECK constraint), so
    /// every listing with an availability is active and the per-column frequencies are enough.
    /// Each group is derived so that its parts add up exactly to the estimated whole; active
    /// listings whose availability is missing from the sampled most common values count as
    /// without availability.
    fn estimate(&self) -> AdminOverviewProductListings {
        let total = self.estimated_total;
        let availability = shares(total, self.availability_frequencies);
        let with_availability: u64 = availability.iter().sum();
        let active = share(total, self.active_frequency).clamp(with_availability, total);

        AdminOverviewProductListings {
            total,
            by_lifecycle: AdminOverviewProductListingLifecycleCounts {
                active,
                withdrawn: total - active,
            },
            active_availability: availability_counts(availability),
            active_without_availability: active - with_availability,
        }
    }
}

/// Orders sampled most-common-value frequencies like `known_values`; absent values have none.
fn frequencies<const N: usize>(
    column: &'static str,
    known_values: &[&str; N],
    values: Option<Vec<String>>,
    frequencies: Option<Vec<f64>>,
) -> Result<[f64; N], AdminOverviewRowMappingError> {
    let values = values.unwrap_or_default();
    let frequencies = frequencies.unwrap_or_default();
    if values.len() != frequencies.len() {
        return Err(AdminOverviewRowMappingError::Statistics {
            column,
            reason: "most common values and frequencies differ in length",
        });
    }
    let mut ordered = [0.0; N];
    for (value, frequency) in values.iter().zip(frequencies) {
        let Some(index) = known_values.iter().position(|known| known == value) else {
            return Err(AdminOverviewRowMappingError::Statistics {
                column,
                reason: "unknown most common value",
            });
        };
        if !(0.0..=1.0).contains(&frequency) {
            return Err(AdminOverviewRowMappingError::Statistics {
                column,
                reason: "frequency outside 0..=1",
            });
        }
        ordered[index] = frequency;
    }
    if ordered.iter().sum::<f64>() > 1.0 + FREQUENCY_SUM_TOLERANCE {
        return Err(AdminOverviewRowMappingError::Statistics {
            column,
            reason: "frequencies sum above 1",
        });
    }
    Ok(ordered)
}

/// Splits `total` by cumulative frequency, so the shares never sum above `total`.
fn shares<const N: usize>(total: u64, frequencies: [f64; N]) -> [u64; N] {
    let mut cumulative_frequency = 0.0;
    let mut counted = 0;
    frequencies.map(|frequency| {
        cumulative_frequency = f64::min(cumulative_frequency + frequency, 1.0);
        let counted_through = share(total, cumulative_frequency);
        let current = counted_through - counted;
        counted = counted_through;
        current
    })
}

fn share(total: u64, frequency: f64) -> u64 {
    // Listing counts stay far below 2^53, where `f64` represents every integer exactly.
    ((total as f64) * frequency.clamp(0.0, 1.0)).floor() as u64
}

fn availability_counts(counts: [u64; 11]) -> AdminOverviewActiveListingAvailabilityCounts {
    let [
        available,
        in_stock,
        limited_availability,
        back_order,
        made_to_order,
        pre_order,
        pre_sale,
        unavailable,
        reserved,
        out_of_stock,
        sold_out,
    ] = counts;
    AdminOverviewActiveListingAvailabilityCounts {
        available,
        in_stock,
        limited_availability,
        back_order,
        made_to_order,
        pre_order,
        pre_sale,
        unavailable,
        reserved,
        out_of_stock,
        sold_out,
    }
}

impl TryFrom<ProductListingCountsRow> for AdminOverviewProductListings {
    type Error = AdminOverviewRowMappingError;

    fn try_from(row: ProductListingCountsRow) -> Result<Self, Self::Error> {
        Ok(Self {
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zero_counts_row() -> ProductListingCountsRow {
        ProductListingCountsRow {
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
        }
    }

    fn analyzed_row(estimated_total: i64) -> ProductListingStatisticsRow {
        ProductListingStatisticsRow {
            estimated_total: Some(estimated_total),
            lifecycle_analyzed: true,
            lifecycle_values: Some(vec!["ACTIVE".to_owned(), "WITHDRAWN".to_owned()]),
            lifecycle_frequencies: Some(vec![0.875, 0.125]),
            availability_analyzed: true,
            availability_values: Some(vec![
                "AVAILABLE".to_owned(),
                "SOLD_OUT".to_owned(),
                "PRE_SALE".to_owned(),
            ]),
            availability_frequencies: Some(vec![0.5, 0.25, 0.007_812_5]),
        }
    }

    fn statistics(row: ProductListingStatisticsRow) -> ProductListingStatistics {
        match row.count_method() {
            Ok(CountMethod::Estimate(statistics)) => statistics,
            other => panic!("expected an estimate, got {other:?}"),
        }
    }

    fn assert_statistics_error(row: ProductListingStatisticsRow, expected_reason: &str) {
        let source = match row.count_method() {
            Err(AdminOverviewReadError::InvalidReadModel { source }) => source,
            other => panic!("expected invalid statistics, got {other:?}"),
        };
        match source.downcast_ref::<AdminOverviewRowMappingError>() {
            Some(AdminOverviewRowMappingError::Statistics { reason, .. }) => {
                assert_eq!(expected_reason, *reason)
            }
            other => panic!("expected statistics error, got {other:?}"),
        }
    }

    #[test]
    fn should_reject_negative_persisted_product_listing_count() {
        let result = AdminOverviewProductListings::try_from(ProductListingCountsRow {
            active_sold_out: -1,
            ..zero_counts_row()
        });

        assert!(matches!(
            result,
            Err(AdminOverviewRowMappingError::Count {
                field: "active_sold_out",
                ..
            })
        ));
    }

    #[test]
    fn should_estimate_breakdown_from_frequencies_when_table_is_large() {
        let estimate = statistics(analyzed_row(1_000_000)).estimate();

        assert_eq!(
            AdminOverviewProductListings {
                total: 1_000_000,
                by_lifecycle: AdminOverviewProductListingLifecycleCounts {
                    active: 875_000,
                    withdrawn: 125_000,
                },
                active_availability: AdminOverviewActiveListingAvailabilityCounts {
                    available: 500_000,
                    sold_out: 250_000,
                    pre_sale: 7_812,
                    ..Default::default()
                },
                active_without_availability: 117_188,
            },
            estimate
        );
    }

    #[test]
    fn should_count_exactly_when_table_is_below_limit_even_with_invalid_statistics() {
        let row = ProductListingStatisticsRow {
            availability_values: Some(vec!["DISCONTINUED".to_owned()]),
            availability_frequencies: Some(vec![0.1]),
            ..analyzed_row(99_999)
        };

        assert!(matches!(row.count_method(), Ok(CountMethod::Exact)));
    }

    #[test]
    fn should_count_exactly_when_table_was_never_vacuumed_or_analyzed() {
        let row = ProductListingStatisticsRow {
            estimated_total: None,
            ..analyzed_row(1_000_000)
        };

        assert!(matches!(row.count_method(), Ok(CountMethod::Exact)));
    }

    #[test]
    fn should_be_temporarily_unavailable_when_large_table_lacks_column_statistics() {
        let row = ProductListingStatisticsRow {
            availability_analyzed: false,
            availability_values: None,
            availability_frequencies: None,
            ..analyzed_row(1_000_000)
        };

        let source = match row.count_method() {
            Err(AdminOverviewReadError::TemporarilyUnavailable { source }) => source,
            other => panic!("expected temporarily unavailable, got {other:?}"),
        };
        assert!(source.downcast_ref::<MissingColumnStatistics>().is_some());
    }

    #[test]
    fn should_count_every_active_listing_as_without_availability_when_none_was_sampled() {
        let estimate = statistics(ProductListingStatisticsRow {
            availability_values: None,
            availability_frequencies: None,
            ..analyzed_row(200_000)
        })
        .estimate();

        assert_eq!(175_000, estimate.by_lifecycle.active);
        assert_eq!(175_000, estimate.active_without_availability);
        assert_eq!(
            AdminOverviewActiveListingAvailabilityCounts::default(),
            estimate.active_availability
        );
    }

    #[test]
    fn should_keep_parts_summing_to_whole_when_sampled_frequencies_disagree() {
        let estimate = statistics(ProductListingStatisticsRow {
            lifecycle_frequencies: Some(vec![0.3, 0.7]),
            availability_values: Some(vec!["AVAILABLE".to_owned(), "IN_STOCK".to_owned()]),
            availability_frequencies: Some(vec![0.333_333_3, 0.333_333_3]),
            ..analyzed_row(1_234_567)
        })
        .estimate();

        let availability = &estimate.active_availability;
        let with_availability = availability.available + availability.in_stock;
        assert_eq!(
            estimate.total,
            estimate.by_lifecycle.active + estimate.by_lifecycle.withdrawn
        );
        assert_eq!(
            estimate.by_lifecycle.active,
            with_availability + estimate.active_without_availability
        );
        assert_eq!(with_availability, estimate.by_lifecycle.active);
    }

    #[test]
    fn should_reject_unknown_most_common_value() {
        assert_statistics_error(
            ProductListingStatisticsRow {
                availability_values: Some(vec!["DISCONTINUED".to_owned()]),
                availability_frequencies: Some(vec![0.1]),
                ..analyzed_row(1_000_000)
            },
            "unknown most common value",
        );
    }

    #[test]
    fn should_reject_inconsistent_frequencies() {
        assert_statistics_error(
            ProductListingStatisticsRow {
                lifecycle_frequencies: Some(vec![0.875]),
                ..analyzed_row(1_000_000)
            },
            "most common values and frequencies differ in length",
        );
        assert_statistics_error(
            ProductListingStatisticsRow {
                lifecycle_frequencies: Some(vec![0.875, f64::NAN]),
                ..analyzed_row(1_000_000)
            },
            "frequency outside 0..=1",
        );
        assert_statistics_error(
            ProductListingStatisticsRow {
                lifecycle_frequencies: Some(vec![0.875, 0.25]),
                ..analyzed_row(1_000_000)
            },
            "frequencies sum above 1",
        );
    }

    #[test]
    fn should_reject_negative_estimated_total() {
        let source = match analyzed_row(-1).count_method() {
            Err(AdminOverviewReadError::InvalidReadModel { source }) => source,
            other => panic!("expected invalid statistics, got {other:?}"),
        };

        assert!(matches!(
            source.downcast_ref::<AdminOverviewRowMappingError>(),
            Some(AdminOverviewRowMappingError::Count {
                field: "estimated_product_listings_total",
                ..
            })
        ));
    }
}
