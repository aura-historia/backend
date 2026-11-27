//! `GET /api/listing-sources/{id}/held-removals`: the product URLs of one ListingSource whose
//! removal the mass-removal guard held back, one window at a time, so an operator sees which
//! listings a confirmation would remove. The contract is in `docs/crawler/operations.md`
//! (review service) and the guard in `docs/crawler/scheduling-and-retries.md` (mass-removal
//! guard).

use crate::review::model::{HeldRemovalUrl, ListPage, ListWindow};
use crate::scraper::removal_guard::MASS_REMOVAL_HELD_ERROR_KIND;
use listing_source_core::ListingSourceId;
use sqlx::{PgPool, Row};

/// How many characters of the guard's message a held URL carries.
const HELD_MESSAGE_CHARS: i32 = 200;

/// Reads the held removals of a ListingSource.
#[derive(Clone)]
pub struct HeldRemovalReader {
    pool: PgPool,
}

impl HeldRemovalReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The window of `listing_source_id`'s product URLs whose removal is held, the most recently
    /// held first, with how many are held in all; the same URLs the overview's
    /// `held_removal_urls` counts.
    pub async fn list(
        &self,
        listing_source_id: ListingSourceId,
        window: ListWindow,
    ) -> Result<ListPage<HeldRemovalUrl>, sqlx::Error> {
        let rows = sqlx::query(HELD_REMOVALS)
            .bind(listing_source_id.as_uuid())
            .bind(MASS_REMOVAL_HELD_ERROR_KIND)
            .bind(i64::try_from(window.offset()).unwrap_or(i64::MAX))
            .bind(i64::try_from(window.limit()).unwrap_or(i64::MAX))
            .bind(HELD_MESSAGE_CHARS)
            .fetch_all(&self.pool)
            .await?;
        let Some(total) = rows.first().map(|row| row.try_get("total")).transpose()? else {
            return Err(sqlx::Error::RowNotFound);
        };
        let mut items = Vec::with_capacity(rows.len());
        for row in &rows {
            if let Some(url) = row.try_get::<Option<String>, _>("url")? {
                items.push(HeldRemovalUrl {
                    url,
                    held_since: row.try_get("held_since")?,
                    title: row.try_get("title")?,
                    source_listing_id: row.try_get("source_listing_id")?,
                    status_code: row.try_get("status_code")?,
                    message: row.try_get("message")?,
                });
            }
        }
        Ok(ListPage::new(items, total, window))
    }
}

/// The held URLs of a ListingSource in a window. Binds `$1`, the ListingSource, `$2`, the error
/// kind of a held removal, `$3`, the rows to skip, `$4`, the most rows to answer, and `$5`, the
/// characters of the message to keep. Every row carries the total; an empty window is one row
/// with the total and no URL.
const HELD_REMOVALS: &str = "
    WITH held AS (
        SELECT url, last_scraped, last_captured_title, last_source_listing_id,
               last_status_code, last_error_message
        FROM listing_source_urls
        WHERE listing_source_id = $1
          AND url_class = 'product'
          AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
          AND last_error_kind = $2
    ), ranked AS (
        SELECT held.*, ROW_NUMBER() OVER (ORDER BY last_scraped DESC NULLS LAST, url) AS position
        FROM held
    )
    SELECT t.total, p.url, p.last_scraped AS held_since, p.last_captured_title AS title,
           p.last_source_listing_id AS source_listing_id, p.last_status_code AS status_code,
           left(p.last_error_message, $5) AS message
    FROM (SELECT COUNT(*) AS total FROM held) t
    LEFT JOIN (SELECT * FROM ranked WHERE position > $3 AND position <= $3 + $4) p ON TRUE
    ORDER BY p.position";
