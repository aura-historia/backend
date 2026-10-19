use crate::readers::PublicCatalogueOverviewRow;
use application::error::box_error;
use platform_postgres::SqlxTransaction;
use product_listing_service::{
    ports::{
        PublicCatalogueOverviewRefreshError, PublicCatalogueOverviewRefresher,
        PublicCatalogueOverviewRefresherFactory,
    },
    use_cases::{
        commands::refresh_public_catalogue_overview::PublicCatalogueOverviewRefresh,
        queries::get_public_catalogue_overview::PublicCatalogueOverview,
    },
};

/// Scans every live listing once, so it is far above the Lambda session default; it must
/// stay below the scheduled maintenance Lambda timeout.
pub const PUBLIC_CATALOGUE_OVERVIEW_REFRESH_STATEMENT_TIMEOUT: &str = "480s";

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPublicCatalogueOverviewRefresherFactory;

struct SqlxPublicCatalogueOverviewRefresher<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct RefreshRow {
    #[sqlx(flatten)]
    overview: PublicCatalogueOverviewRow,
    stored: bool,
}

impl SqlxPublicCatalogueOverviewRefresherFactory {
    pub fn new() -> Self {
        Self
    }
}

impl PublicCatalogueOverviewRefresherFactory<SqlxTransaction>
    for SqlxPublicCatalogueOverviewRefresherFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PublicCatalogueOverviewRefresher + 'tx {
        SqlxPublicCatalogueOverviewRefresher {
            connection: tx.connection(),
        }
    }
}

// One statement fixes the snapshot and `as_of`. Grouping first keeps the distinct
// counts over a few source/language/country combinations instead of sorting every
// live listing per metric; language sets are combined only after that aggregation.
// The window is a fixed 168 hours so the session time zone cannot change its length.
// The row lock is taken only when the aggregate finishes, and an older calculation
// never replaces a newer one from a concurrent refresh.
const REFRESH_PUBLIC_CATALOGUE_OVERVIEW_SQL: &str = r#"
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
),
calculated AS (
    SELECT
        statement_timestamp() AS as_of,
        COUNT(DISTINCT listing_source_id)::bigint AS sources_indexed,
        COALESCE(SUM(live_objects), 0)::bigint AS live_objects,
        COUNT(DISTINCT inventory_country)::bigint AS countries,
        (SELECT COUNT(*) FROM languages)::bigint AS languages,
        COALESCE(SUM(new_objects), 0)::bigint AS new_objects_last_7_days
    FROM live_groups
),
stored AS (
    INSERT INTO public_catalogue_overview (
        as_of, sources_indexed, live_objects, countries, languages, new_objects_last_7_days
    )
    SELECT as_of, sources_indexed, live_objects, countries, languages, new_objects_last_7_days
    FROM calculated
    ON CONFLICT (singleton) DO UPDATE SET
        as_of = EXCLUDED.as_of,
        sources_indexed = EXCLUDED.sources_indexed,
        live_objects = EXCLUDED.live_objects,
        countries = EXCLUDED.countries,
        languages = EXCLUDED.languages,
        new_objects_last_7_days = EXCLUDED.new_objects_last_7_days
    WHERE public_catalogue_overview.as_of < EXCLUDED.as_of
    RETURNING 1
)
SELECT
    as_of, sources_indexed, live_objects, countries, languages, new_objects_last_7_days,
    EXISTS (SELECT 1 FROM stored) AS stored
FROM calculated
"#;

#[async_trait::async_trait]
impl PublicCatalogueOverviewRefresher for SqlxPublicCatalogueOverviewRefresher<'_> {
    async fn refresh(
        &mut self,
    ) -> Result<PublicCatalogueOverviewRefresh, PublicCatalogueOverviewRefreshError> {
        sqlx::query("SELECT set_config('statement_timeout', $1, true)")
            .bind(PUBLIC_CATALOGUE_OVERVIEW_REFRESH_STATEMENT_TIMEOUT)
            .execute(&mut *self.connection)
            .await
            .map_err(temporary)?;
        let row = sqlx::query_as::<_, RefreshRow>(REFRESH_PUBLIC_CATALOGUE_OVERVIEW_SQL)
            .fetch_one(&mut *self.connection)
            .await
            .map_err(temporary)?;

        Ok(PublicCatalogueOverviewRefresh {
            overview: PublicCatalogueOverview::try_from(row.overview).map_err(|source| {
                PublicCatalogueOverviewRefreshError::InvalidReadModel {
                    source: box_error(source),
                }
            })?,
            stored: row.stored,
        })
    }
}

fn temporary(source: sqlx::Error) -> PublicCatalogueOverviewRefreshError {
    PublicCatalogueOverviewRefreshError::TemporarilyUnavailable {
        source: box_error(source),
    }
}
