use crate::mapping::{CONTENT_NOTICE_COLUMNS, ContentNoticeRow, MappingError, content_notice};
use crate::sql_error::is_temporary;
use application::error::box_error;
use content_notice_core::content_notice_id::ContentNoticeId;
use content_notice_service::{
    ports::{
        ContentNoticeDetailsReadError, ContentNoticeDetailsReader,
        ContentNoticeDetailsReaderFactory,
    },
    use_cases::queries::get_admin_content_notice::{
        AdminContentNoticeDetails, ContentNoticeReceiptDelivery, ContentNoticeReceiptStatus,
        CurrentContentNoticeTarget, CurrentListingSource, CurrentProductListing,
    },
};
use domain_primitives::versioned::Versioned;
use listing_source_core::{
    InvalidListingIngestionMethod, InvalidListingSourceSlug, ListingIngestionMethod,
    ListingSourceName, ListingSourceNameError, ListingSourceSlugId,
};
use notification_core::external_email::{ExternalEmailOrigin, ExternalEmailTemplate};
use platform_postgres::SqlxTransaction;
use product_listing_core::listing_lifecycle::ListingLifecycle;
use sqlx::{AssertSqlSafe, PgConnection};
use time::OffsetDateTime;

/// One statement over constant SQL: the notice, joined to its target as it is now and to the
/// delivery of its receipt email. The joins are left joins because a notice has no foreign keys
/// and outlives its target, and because its receipt may be missing. The partner flag follows the
/// public listing source details: the operator party holds an ACTIVE partnership. The receipt is
/// found by its origin, which is unique per template (`docs/arch.md` allows readers this SQL
/// coupling to the notification context's tables).
fn details_sql() -> String {
    format!(
        "WITH notice AS (SELECT {CONTENT_NOTICE_COLUMNS}, created, updated FROM content_notices WHERE content_notice_id = $1) \
         SELECT notice.*, \
         s.listing_source_id IS NOT NULL AS current_listing_source_exists, \
         s.name AS current_listing_source_name, \
         s.listing_source_slug_id AS current_listing_source_slug_id, \
         ARRAY(SELECT m.ingestion_method FROM listing_source_ingestion_methods m WHERE m.listing_source_id = s.listing_source_id ORDER BY m.ingestion_method) AS current_ingestion_methods, \
         EXISTS (SELECT 1 FROM partnerships operator_partnership WHERE operator_partnership.party_id = s.operator_party_id AND operator_partnership.business_state = 'ACTIVE') AS current_listing_source_partner, \
         p.lifecycle AS current_product_listing_lifecycle, \
         receipt.external_email_message_id IS NOT NULL AS receipt_exists, \
         receipt.status AS receipt_status, \
         receipt.attempt_count AS receipt_attempts, \
         receipt.delivered_at AS receipt_delivered_at \
         FROM notice \
         LEFT JOIN listing_sources s ON s.listing_source_id = notice.listing_source_id \
         LEFT JOIN product_listings p ON p.product_listing_id = notice.product_listing_id \
         LEFT JOIN LATERAL (SELECT m.external_email_message_id, d.status, d.attempt_count, d.delivered_at \
             FROM external_email_messages m \
             LEFT JOIN notification_deliveries d ON d.external_email_message_id = m.external_email_message_id \
             WHERE m.origin_type = $2 AND m.origin_id = notice.content_notice_id AND m.template = $3) receipt ON true"
    )
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxContentNoticeDetailsReaderFactory;

impl SqlxContentNoticeDetailsReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

struct Reader<'tx> {
    connection: &'tx mut PgConnection,
}

impl ContentNoticeDetailsReaderFactory<SqlxTransaction> for SqlxContentNoticeDetailsReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ContentNoticeDetailsReader + 'tx {
        Reader {
            connection: tx.connection(),
        }
    }
}

#[derive(sqlx::FromRow)]
struct ContentNoticeDetailsRow {
    #[sqlx(flatten)]
    notice: ContentNoticeRow,
    created: OffsetDateTime,
    updated: OffsetDateTime,
    current_listing_source_exists: bool,
    current_listing_source_name: Option<String>,
    current_listing_source_slug_id: Option<String>,
    current_ingestion_methods: Vec<String>,
    current_listing_source_partner: bool,
    current_product_listing_lifecycle: Option<String>,
    receipt_exists: bool,
    receipt_status: Option<String>,
    receipt_attempts: Option<i32>,
    receipt_delivered_at: Option<OffsetDateTime>,
}

/// Why a joined row is not a valid detail view. Variants never carry the stored values.
#[derive(Debug, thiserror::Error)]
enum DetailsMappingError {
    #[error("invalid persisted content notice")]
    Notice(#[source] MappingError),
    #[error("current listing source row is incomplete")]
    IncompleteListingSource,
    #[error("invalid current ListingSource name persisted")]
    ListingSourceName(#[source] ListingSourceNameError),
    #[error("invalid current ListingSource slug persisted")]
    ListingSourceSlug(#[source] InvalidListingSourceSlug),
    #[error("invalid current ListingSource ingestion method persisted")]
    IngestionMethod(#[source] InvalidListingIngestionMethod),
    #[error("invalid current ProductListing lifecycle persisted")]
    Lifecycle,
    #[error("receipt email message has no delivery")]
    ReceiptWithoutDelivery,
    #[error("invalid receipt delivery status persisted")]
    ReceiptStatus,
    #[error("invalid receipt delivery attempt count persisted")]
    ReceiptAttempts,
    #[error("receipt delivery time does not match its status")]
    ReceiptDeliveredAt,
}

#[async_trait::async_trait]
impl ContentNoticeDetailsReader for Reader<'_> {
    async fn find_by_id(
        &mut self,
        content_notice_id: ContentNoticeId,
    ) -> Result<Option<AdminContentNoticeDetails>, ContentNoticeDetailsReadError> {
        let origin = ExternalEmailOrigin::ContentNotice(content_notice_id);
        sqlx::query_as::<_, ContentNoticeDetailsRow>(AssertSqlSafe(details_sql()))
            .bind(content_notice_id.into_uuid())
            .bind(origin.origin_type())
            .bind(ExternalEmailTemplate::ContentNoticeReceipt.as_str())
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(|source| {
                if is_temporary(&source) {
                    ContentNoticeDetailsReadError::TemporarilyUnavailable {
                        source: box_error(source),
                    }
                } else {
                    ContentNoticeDetailsReadError::InvalidReadModel {
                        source: box_error(source),
                    }
                }
            })?
            .map(details)
            .transpose()
            .map_err(|source| ContentNoticeDetailsReadError::InvalidReadModel {
                source: box_error(source),
            })
    }
}

fn details(row: ContentNoticeDetailsRow) -> Result<AdminContentNoticeDetails, DetailsMappingError> {
    let Versioned {
        value: notice,
        version,
    } = content_notice(row.notice).map_err(DetailsMappingError::Notice)?;
    let listing_source = row
        .current_listing_source_exists
        .then(|| {
            current_listing_source(
                row.current_listing_source_name,
                row.current_listing_source_slug_id,
                row.current_ingestion_methods,
                row.current_listing_source_partner,
            )
        })
        .transpose()?;
    let product_listing = current_product_listing(row.current_product_listing_lifecycle)?;
    let receipt = row
        .receipt_exists
        .then(|| {
            receipt_delivery(
                row.receipt_status,
                row.receipt_attempts,
                row.receipt_delivered_at,
            )
        })
        .transpose()?;
    Ok(AdminContentNoticeDetails {
        notice: notice.into(),
        version,
        created: row.created,
        updated: row.updated,
        current_target: CurrentContentNoticeTarget {
            listing_source,
            product_listing,
        },
        receipt,
    })
}

/// The delivery of an existing receipt message. Every message has exactly one delivery, which
/// carries a delivery time exactly when it is delivered.
fn receipt_delivery(
    status: Option<String>,
    attempts: Option<i32>,
    delivered_at: Option<OffsetDateTime>,
) -> Result<ContentNoticeReceiptDelivery, DetailsMappingError> {
    let (Some(status), Some(attempts)) = (status, attempts) else {
        return Err(DetailsMappingError::ReceiptWithoutDelivery);
    };
    let status =
        ContentNoticeReceiptStatus::from_code(&status).ok_or(DetailsMappingError::ReceiptStatus)?;
    if (status == ContentNoticeReceiptStatus::Delivered) != delivered_at.is_some() {
        return Err(DetailsMappingError::ReceiptDeliveredAt);
    }
    Ok(ContentNoticeReceiptDelivery {
        status,
        attempts: u32::try_from(attempts).map_err(|_| DetailsMappingError::ReceiptAttempts)?,
        delivered_at,
    })
}

fn current_product_listing(
    lifecycle: Option<String>,
) -> Result<Option<CurrentProductListing>, DetailsMappingError> {
    lifecycle
        .map(|lifecycle| {
            ListingLifecycle::from_code(&lifecycle)
                .map(|lifecycle| CurrentProductListing { lifecycle })
                .ok_or(DetailsMappingError::Lifecycle)
        })
        .transpose()
}

fn current_listing_source(
    name: Option<String>,
    slug_id: Option<String>,
    ingestion_methods: Vec<String>,
    partner: bool,
) -> Result<CurrentListingSource, DetailsMappingError> {
    let (Some(name), Some(slug_id)) = (name, slug_id) else {
        return Err(DetailsMappingError::IncompleteListingSource);
    };
    Ok(CurrentListingSource {
        name: ListingSourceName::try_from(name).map_err(DetailsMappingError::ListingSourceName)?,
        slug_id: ListingSourceSlugId::raw(slug_id)
            .map_err(DetailsMappingError::ListingSourceSlug)?,
        ingestion_methods: ingestion_methods
            .iter()
            .map(|method| method.parse::<ListingIngestionMethod>())
            .collect::<Result<_, _>>()
            .map_err(DetailsMappingError::IngestionMethod)?,
        partner,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_fail_closed_on_invalid_current_listing_source_values() {
        assert!(matches!(
            current_listing_source(Some("Antik".into()), None, Vec::new(), false),
            Err(DetailsMappingError::IncompleteListingSource)
        ));
        assert!(matches!(
            current_listing_source(
                Some("Antik".into()),
                Some("Not a slug".into()),
                Vec::new(),
                false
            ),
            Err(DetailsMappingError::ListingSourceSlug(_))
        ));
        assert!(matches!(
            current_listing_source(
                Some("Antik".into()),
                Some("antik".into()),
                vec!["CARRIER_PIGEON".into()],
                false
            ),
            Err(DetailsMappingError::IngestionMethod(_))
        ));
    }

    #[test]
    fn should_map_current_lifecycle_and_fail_closed_on_an_unknown_one() {
        assert_eq!(
            Some(CurrentProductListing {
                lifecycle: ListingLifecycle::Withdrawn
            }),
            current_product_listing(Some("WITHDRAWN".into())).unwrap()
        );
        assert_eq!(None, current_product_listing(None).unwrap());
        assert!(matches!(
            current_product_listing(Some("ARCHIVED".into())),
            Err(DetailsMappingError::Lifecycle)
        ));
    }

    #[test]
    fn should_map_current_listing_source_in_stored_order() {
        let source = current_listing_source(
            Some("Antik".into()),
            Some("antik".into()),
            vec!["SHOPIFY".into(), "WEB_CRAWL".into()],
            true,
        )
        .unwrap();

        assert_eq!(
            vec![
                ListingIngestionMethod::Shopify,
                ListingIngestionMethod::WebCrawl
            ],
            source.ingestion_methods
        );
        assert_eq!("antik", source.slug_id.as_ref());
        assert!(source.partner);
    }

    #[test]
    fn should_map_receipt_delivery_and_fail_closed_on_invalid_values() {
        let delivered_at = OffsetDateTime::UNIX_EPOCH;
        assert_eq!(
            ContentNoticeReceiptDelivery {
                status: ContentNoticeReceiptStatus::Delivered,
                attempts: 1,
                delivered_at: Some(delivered_at),
            },
            receipt_delivery(Some("DELIVERED".into()), Some(1), Some(delivered_at)).unwrap()
        );
        assert_eq!(
            ContentNoticeReceiptStatus::Failed,
            receipt_delivery(Some("FAILED".into()), Some(5), None)
                .unwrap()
                .status
        );
        assert!(matches!(
            receipt_delivery(None, None, None),
            Err(DetailsMappingError::ReceiptWithoutDelivery)
        ));
        assert!(matches!(
            receipt_delivery(Some("SUPPRESSED".into()), Some(0), None),
            Err(DetailsMappingError::ReceiptStatus)
        ));
        assert!(matches!(
            receipt_delivery(Some("PENDING".into()), Some(-1), None),
            Err(DetailsMappingError::ReceiptAttempts)
        ));
        for (status, delivered_at) in [("DELIVERED", None), ("PENDING", Some(delivered_at))] {
            assert!(matches!(
                receipt_delivery(Some(status.into()), Some(1), delivered_at),
                Err(DetailsMappingError::ReceiptDeliveredAt)
            ));
        }
    }
}
