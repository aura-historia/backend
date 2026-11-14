use application::error::box_error;
use content_notice_core::{
    bounded_text::BoundedTextError,
    content_notice_target::{
        ListingSourceSnapshot, ProductListingSnapshot, ProductListingTitleSnapshot,
    },
};
use content_notice_service::ports::{
    ContentNoticeTargetReadError, ContentNoticeTargetReader, ContentNoticeTargetReaderFactory,
    ListingSourceTarget, ProductListingTarget,
};
use domain_primitives::object_id::ObjectIdError;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceNameError};
use platform_postgres::SqlxTransaction;
use product_listing_core::{
    listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
};
use sqlx::PgConnection;
use url::Url;

// The partner flag follows the public listing source details: the operator party holds an
// ACTIVE partnership.
const LISTING_SOURCE_SQL: &str = "SELECT s.listing_source_id, s.name AS listing_source_name, s.url AS listing_source_url, operator_partnership.partnership_id IS NOT NULL AS listing_source_partner FROM listing_sources s LEFT JOIN partnerships operator_partnership ON operator_partnership.party_id = s.operator_party_id AND operator_partnership.business_state = 'ACTIVE' WHERE s.listing_source_id = $1";
const PRODUCT_LISTING_SQL: &str = "SELECT p.product_listing_id, p.lifecycle, p.title_text AS product_listing_title, p.url AS product_listing_url, s.listing_source_id, s.name AS listing_source_name, s.url AS listing_source_url, operator_partnership.partnership_id IS NOT NULL AS listing_source_partner FROM product_listings p JOIN listing_sources s ON s.listing_source_id = p.listing_source_id LEFT JOIN partnerships operator_partnership ON operator_partnership.party_id = s.operator_party_id AND operator_partnership.business_state = 'ACTIVE' WHERE p.product_listing_id = $1";

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxContentNoticeTargetReaderFactory;

impl SqlxContentNoticeTargetReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

struct Reader<'tx> {
    connection: &'tx mut PgConnection,
}

impl ContentNoticeTargetReaderFactory<SqlxTransaction> for SqlxContentNoticeTargetReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ContentNoticeTargetReader + 'tx {
        Reader {
            connection: tx.connection(),
        }
    }
}

#[derive(sqlx::FromRow)]
struct ListingSourceRow {
    listing_source_id: uuid::Uuid,
    listing_source_name: String,
    listing_source_url: Option<String>,
    listing_source_partner: bool,
}

#[derive(sqlx::FromRow)]
struct ProductListingRow {
    product_listing_id: uuid::Uuid,
    lifecycle: String,
    product_listing_title: Option<String>,
    product_listing_url: String,
    #[sqlx(flatten)]
    listing_source: ListingSourceRow,
}

/// Why a stored target cannot be snapshotted. Variants never carry the stored values.
#[derive(Debug, thiserror::Error)]
enum TargetMappingError {
    #[error("invalid ListingSource ID persisted")]
    ListingSourceId(#[source] ObjectIdError),
    #[error("invalid ListingSource name persisted")]
    ListingSourceName(#[source] ListingSourceNameError),
    #[error("invalid ListingSource URL persisted")]
    ListingSourceUrl(#[source] url::ParseError),
    #[error("invalid ProductListing ID persisted")]
    ProductListingId(#[source] ObjectIdError),
    #[error("invalid ProductListing lifecycle persisted")]
    Lifecycle,
    #[error("invalid ProductListing title persisted")]
    ProductListingTitle(#[source] BoundedTextError),
    #[error("invalid ProductListing URL persisted")]
    ProductListingUrl(#[source] url::ParseError),
}

#[async_trait::async_trait]
impl ContentNoticeTargetReader for Reader<'_> {
    async fn find_listing_source(
        &mut self,
        listing_source_id: ListingSourceId,
    ) -> Result<Option<ListingSourceTarget>, ContentNoticeTargetReadError> {
        sqlx::query_as::<_, ListingSourceRow>(LISTING_SOURCE_SQL)
            .bind(listing_source_id.into_uuid())
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(temporary)?
            .map(listing_source_target)
            .transpose()
            .map_err(invalid)
    }

    async fn find_product_listing(
        &mut self,
        product_listing_id: ProductListingId,
    ) -> Result<Option<ProductListingTarget>, ContentNoticeTargetReadError> {
        sqlx::query_as::<_, ProductListingRow>(PRODUCT_LISTING_SQL)
            .bind(product_listing_id.into_uuid())
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(temporary)?
            .map(product_listing_target)
            .transpose()
            .map_err(invalid)
    }
}

fn listing_source_target(row: ListingSourceRow) -> Result<ListingSourceTarget, TargetMappingError> {
    Ok(ListingSourceTarget {
        listing_source_id: ListingSourceId::try_from(row.listing_source_id)
            .map_err(TargetMappingError::ListingSourceId)?,
        snapshot: ListingSourceSnapshot {
            name: ListingSourceName::try_from(row.listing_source_name)
                .map_err(TargetMappingError::ListingSourceName)?,
            url: row
                .listing_source_url
                .as_deref()
                .map(Url::parse)
                .transpose()
                .map_err(TargetMappingError::ListingSourceUrl)?,
            partner: row.listing_source_partner,
        },
    })
}

fn product_listing_target(
    row: ProductListingRow,
) -> Result<ProductListingTarget, TargetMappingError> {
    Ok(ProductListingTarget {
        product_listing_id: ProductListingId::try_from(row.product_listing_id)
            .map_err(TargetMappingError::ProductListingId)?,
        lifecycle: ListingLifecycle::from_code(&row.lifecycle)
            .ok_or(TargetMappingError::Lifecycle)?,
        snapshot: ProductListingSnapshot {
            title: row
                .product_listing_title
                .map(ProductListingTitleSnapshot::try_from)
                .transpose()
                .map_err(TargetMappingError::ProductListingTitle)?,
            url: Url::parse(&row.product_listing_url)
                .map_err(TargetMappingError::ProductListingUrl)?,
        },
        listing_source: listing_source_target(row.listing_source)?,
    })
}

fn temporary(error: sqlx::Error) -> ContentNoticeTargetReadError {
    ContentNoticeTargetReadError::TemporarilyUnavailable {
        source: box_error(error),
    }
}

fn invalid(error: TargetMappingError) -> ContentNoticeTargetReadError {
    ContentNoticeTargetReadError::InvalidReadModel {
        source: box_error(error),
    }
}
