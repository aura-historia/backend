//! Re-arms the pages of a ListingSource that a new product-schema configuration may now decide
//! differently, so they are scraped again at once instead of at their next scrape interval.

use super::ReviewRepositoryError;
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use listing_source_core::ListingSourceId;
use tracing::info;

/// Makes due at once, inside the transaction that makes `live_removed_page_schemas` and the
/// ListingSource's new schemas live, every product page of the ListingSource that:
///
/// - recorded `MissingRequiredValue`: the new schemas may find the value, and only a scrape
///   tells, which would otherwise wait for the page's scrape interval; and
/// - was last captured as removed by a removed-page template the live list no longer holds
///   unchanged, because the template was retired or narrowed: the page may be a product page
///   the old template withdrew, and only a scrape recaptures it.
///
/// A re-armed page is due as if never scraped, unless a retry cooldown a later fetch outcome set,
/// such as a bot challenge's, still holds it back; its outcome and capture stay until the scrape
/// replaces them. A retired `DUPLICATE` URL is never re-armed. Returns how many pages it
/// re-armed.
pub(super) async fn rearm_pages_for_new_configuration(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    listing_source_id: &ListingSourceId,
    live_removed_page_schemas: &[RemovedPageSchema],
) -> Result<u64, ReviewRepositoryError> {
    let live_templates = serde_json::to_value(live_removed_page_schemas)?;
    let rearmed = sqlx::query(
        "UPDATE listing_source_urls \
         SET last_scraped = NULL, updated = NOW() \
         WHERE listing_source_id = $1 \
           AND url_class = 'product' \
           AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD') \
           AND (last_error_kind = 'MissingRequiredValue' \
                OR (removed_by_templates IS NOT NULL \
                    AND EXISTS ( \
                        SELECT 1 FROM jsonb_array_elements(removed_by_templates) AS used(template) \
                        WHERE NOT EXISTS ( \
                            SELECT 1 FROM jsonb_array_elements($2::jsonb) AS live(template) \
                            WHERE live.template = used.template))))",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(live_templates)
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    if rearmed > 0 {
        info!(
            listing_source_id = %listing_source_id,
            rearmed_pages = rearmed,
            "Re-armed pages the new product schema configuration may decide differently"
        );
    }
    Ok(rearmed)
}
