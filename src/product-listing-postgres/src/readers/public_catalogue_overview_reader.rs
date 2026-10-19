use application::error::box_error;
use platform_postgres::SqlxTransaction;
use product_listing_service::{
    ports::{
        PublicCatalogueOverviewReadError, PublicCatalogueOverviewReader,
        PublicCatalogueOverviewReaderFactory,
    },
    use_cases::queries::get_public_catalogue_overview::PublicCatalogueOverview,
};
use time::{OffsetDateTime, UtcOffset};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPublicCatalogueOverviewReaderFactory;

struct SqlxPublicCatalogueOverviewReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct PublicCatalogueOverviewRow {
    as_of: OffsetDateTime,
    sources_indexed: i64,
    live_objects: i64,
    countries: i64,
    languages: i64,
    new_objects_last_7_days: i64,
}

#[derive(Debug, thiserror::Error)]
enum PublicCatalogueOverviewRowMappingError {
    #[error("invalid aggregated {field} count")]
    Count {
        field: &'static str,
        #[source]
        source: std::num::TryFromIntError,
    },
}

impl SqlxPublicCatalogueOverviewReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl PublicCatalogueOverviewReaderFactory<SqlxTransaction>
    for SqlxPublicCatalogueOverviewReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PublicCatalogueOverviewReader + 'tx {
        SqlxPublicCatalogueOverviewReader {
            connection: tx.connection(),
        }
    }
}

// One statement fixes the snapshot and `as_of`. Grouping first keeps the distinct
// counts over a few source/language/country combinations instead of sorting every
// live listing per metric; language sets are combined only after that aggregation.
// The window is a fixed 168 hours so the session time zone cannot change its length.
const PUBLIC_CATALOGUE_OVERVIEW_SQL: &str = r#"
WITH live_groups AS (
    SELECT
        listing.listing_source_id,
        listing.title_language,
        listing.description_language,
        geography.geography #>> '{inventory,country}' AS inventory_country,
        COUNT(*) AS live_objects,
        COUNT(*) FILTER (
            WHERE listing.created > statement_timestamp() - interval '168 hours'
              AND listing.created <= statement_timestamp()
        ) AS new_objects
    FROM product_listings listing
    LEFT JOIN product_listing_geographies geography
      ON geography.product_listing_id = listing.product_listing_id
    WHERE listing.lifecycle = 'ACTIVE'
    GROUP BY 1, 2, 3, 4
),
languages AS (
    SELECT title_language AS language FROM live_groups WHERE title_language IS NOT NULL
    UNION
    SELECT description_language FROM live_groups WHERE description_language IS NOT NULL
)
SELECT
    statement_timestamp() AS as_of,
    COUNT(DISTINCT listing_source_id)::bigint AS sources_indexed,
    COALESCE(SUM(live_objects), 0)::bigint AS live_objects,
    COUNT(DISTINCT inventory_country)::bigint AS countries,
    (SELECT COUNT(*) FROM languages)::bigint AS languages,
    COALESCE(SUM(new_objects), 0)::bigint AS new_objects_last_7_days
FROM live_groups
"#;

#[async_trait::async_trait]
impl PublicCatalogueOverviewReader for SqlxPublicCatalogueOverviewReader<'_> {
    async fn read_overview(
        &mut self,
    ) -> Result<PublicCatalogueOverview, PublicCatalogueOverviewReadError> {
        let row = sqlx::query_as::<_, PublicCatalogueOverviewRow>(PUBLIC_CATALOGUE_OVERVIEW_SQL)
            .fetch_one(&mut *self.connection)
            .await
            .map_err(
                |source| PublicCatalogueOverviewReadError::TemporarilyUnavailable {
                    source: box_error(source),
                },
            )?;

        PublicCatalogueOverview::try_from(row).map_err(|source| {
            PublicCatalogueOverviewReadError::InvalidReadModel {
                source: box_error(source),
            }
        })
    }
}

impl TryFrom<PublicCatalogueOverviewRow> for PublicCatalogueOverview {
    type Error = PublicCatalogueOverviewRowMappingError;

    fn try_from(row: PublicCatalogueOverviewRow) -> Result<Self, Self::Error> {
        let count = |field, value| {
            u64::try_from(value)
                .map_err(|source| PublicCatalogueOverviewRowMappingError::Count { field, source })
        };

        Ok(Self {
            as_of: row.as_of.to_offset(UtcOffset::UTC),
            sources_indexed: count("sources_indexed", row.sources_indexed)?,
            live_objects: count("live_objects", row.live_objects)?,
            countries: count("countries", row.countries)?,
            languages: count("languages", row.languages)?,
            new_objects_last_7_days: count("new_objects_last_7_days", row.new_objects_last_7_days)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AS_OF_UNIX_SECONDS: i64 = 1_792_404_000;

    fn row() -> PublicCatalogueOverviewRow {
        let offset = UtcOffset::from_hms(2, 0, 0)
            .unwrap_or_else(|error| panic!("valid test offset: {error}"));
        PublicCatalogueOverviewRow {
            as_of: OffsetDateTime::from_unix_timestamp(AS_OF_UNIX_SECONDS)
                .unwrap_or_else(|error| panic!("valid test timestamp: {error}"))
                .to_offset(offset),
            sources_indexed: 1,
            live_objects: 2,
            countries: 1,
            languages: 2,
            new_objects_last_7_days: 1,
        }
    }

    #[test]
    fn should_normalize_as_of_to_utc() {
        let overview = PublicCatalogueOverview::try_from(row())
            .unwrap_or_else(|error| panic!("valid overview row: {error}"));

        assert_eq!(AS_OF_UNIX_SECONDS, overview.as_of.unix_timestamp());
        assert_eq!(UtcOffset::UTC, overview.as_of.offset());
    }

    #[test]
    fn should_reject_negative_aggregated_count() {
        let result = PublicCatalogueOverview::try_from(PublicCatalogueOverviewRow {
            countries: -1,
            ..row()
        });

        assert!(matches!(
            result,
            Err(PublicCatalogueOverviewRowMappingError::Count {
                field: "countries",
                ..
            })
        ));
    }
}
