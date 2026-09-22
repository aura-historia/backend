use super::public_listing_source::{PublicListingSourceRow, map_public_listing_source};
use application::error::box_error;
use listing_source_core::ListingSourceSlugId;
use listing_source_service::{
    ports::{
        PublicListingSourceDetailsReadError, PublicListingSourceDetailsReader,
        PublicListingSourceDetailsReaderFactory,
    },
    use_cases::queries::public_listing_source::PublicListingSourceSummary,
};
use platform_postgres::SqlxTransaction;

const STATEMENT_TIMEOUT: &str = "150ms";
pub(super) const DETAIL_BY_SLUG_SQL: &str = "SELECT s.listing_source_id, s.listing_source_slug_id, s.name, p.name AS operator_name, s.url, s.image FROM listing_sources s JOIN parties p ON p.party_id = s.operator_party_id WHERE s.listing_source_slug_id = $1";

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPublicListingSourceDetailsReaderFactory;

struct SqlxPublicListingSourceDetailsReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

impl SqlxPublicListingSourceDetailsReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl PublicListingSourceDetailsReaderFactory<SqlxTransaction>
    for SqlxPublicListingSourceDetailsReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PublicListingSourceDetailsReader + 'tx {
        SqlxPublicListingSourceDetailsReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl PublicListingSourceDetailsReader for SqlxPublicListingSourceDetailsReader<'_> {
    async fn find_by_slug(
        &mut self,
        slug_id: &ListingSourceSlugId,
    ) -> Result<Option<PublicListingSourceSummary>, PublicListingSourceDetailsReadError> {
        set_statement_timeout(self.connection).await?;

        sqlx::query_as::<_, PublicListingSourceRow>(DETAIL_BY_SLUG_SQL)
            .bind(slug_id.as_ref())
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(temporary)?
            .map(map_public_listing_source)
            .transpose()
            .map_err(invalid)
    }
}

async fn set_statement_timeout(
    connection: &mut sqlx::PgConnection,
) -> Result<(), PublicListingSourceDetailsReadError> {
    sqlx::query("SELECT set_config('statement_timeout', $1, true)")
        .bind(STATEMENT_TIMEOUT)
        .execute(connection)
        .await
        .map_err(temporary)?;
    Ok(())
}

fn temporary(source: sqlx::Error) -> PublicListingSourceDetailsReadError {
    PublicListingSourceDetailsReadError::TemporarilyUnavailable {
        source: box_error(source),
    }
}

fn invalid(
    source: impl std::error::Error + Send + Sync + 'static,
) -> PublicListingSourceDetailsReadError {
    PublicListingSourceDetailsReadError::InvalidReadModel {
        source: box_error(source),
    }
}
