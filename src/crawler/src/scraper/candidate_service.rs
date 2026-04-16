//! Service for fetching scraper candidates — product URLs that are due for re-scraping.
//!
//! A scraper candidate is a URL stored in `shop_urls` that is due for scraping by recency and
//! retry/state rules. Hash comparison is performed in-memory by the scraper after fetching HTML.
//! Each candidate carries the shop metadata (`shop_id`, `shop_name`, `shop_type`) needed to build
//! an [`UpsertProductCommand`] without an additional lookup, as well as snapshots of the last
//! successfully scraped field values used for change detection.

use async_trait::async_trait;
use common::shop_id::ShopId;
use shop::core::shop_type::ShopType;
use sqlx::PgPool;
use time::OffsetDateTime;
use url::Url;

use crate::scraper::normalization::product::NormalizedProduct;
use crate::service::shop_registration::shop_type_from_db;
use crate::spider::classification::url_metadata::UrlState;

// ---------------------------------------------------------------------------
// ScraperCandidate
// ---------------------------------------------------------------------------

/// A product URL that is eligible for scraping, together with the shop context and the
/// last-scraped field snapshots used to detect whether the product has actually changed.
pub struct ScraperCandidate {
    pub shop_id: ShopId,
    pub shop_name: String,
    pub shop_type: ShopType,
    pub url: Url,
    /// SHA-256 hash of the HTML `<main>` fragment (or full HTML) from the last successful scrape.
    /// Used for quick whole-page change detection before field-level comparison.
    pub last_scraped_hash: Option<String>,

    // --- field-level snapshots -----------------------------------------------
    /// Serialized last known price (e.g. `"EUR 1200.00"`).
    pub last_scraped_price: Option<String>,
    pub last_scraped_price_estimate_min: Option<String>,
    pub last_scraped_price_estimate_max: Option<String>,
    /// Canonical product URL as persisted from the last scrape.
    pub last_scraped_url: Option<String>,
    /// SHA-256 of the sorted image URL list from the last scrape.
    pub last_scraped_images_hash: Option<String>,
    /// ISO 8601 auction start timestamp.
    pub last_scraped_auction_start: Option<String>,
    /// ISO 8601 auction end timestamp.
    pub last_scraped_auction_end: Option<String>,
    /// Normalized product state from the last scrape (e.g. `"AVAILABLE"`).
    pub last_scraped_state: Option<String>,
}

// ---------------------------------------------------------------------------
// Change detection
// ---------------------------------------------------------------------------

/// Snapshot of a [`NormalizedProduct`] serialized to the same TEXT representation used in the
/// database so that it can be compared directly with the `last_scraped_*` columns.
#[derive(Debug)]
pub struct ProductSnapshot {
    pub price: Option<String>,
    pub price_estimate_min: Option<String>,
    pub price_estimate_max: Option<String>,
    pub url: String,
    pub images_hash: String,
    pub auction_start: Option<String>,
    pub auction_end: Option<String>,
    pub state: String,
}

impl ProductSnapshot {
    /// Build a snapshot from a normalised product.  The representations produced here must match
    /// exactly what [`ScraperCandidateServiceImpl::mark_as_scraped`] persists.
    pub fn from_normalized(product: &NormalizedProduct) -> Self {
        use sha2::{Digest, Sha256};

        fn serialize_price(p: &common::price::domain::Price) -> String {
            format!("{:?} {:?}", p.currency, p.monetary_amount)
        }

        let mut sorted_images: Vec<String> =
            product.images.iter().map(|i| i.url.to_string()).collect();
        sorted_images.sort();
        let images_hash = {
            let mut h = Sha256::new();
            for img in &sorted_images {
                h.update(img.as_bytes());
                h.update(b"\n");
            }
            let digest = h.finalize();
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };

        Self {
            price: product.price.as_ref().map(serialize_price),
            price_estimate_min: product.price_estimate_min.as_ref().map(serialize_price),
            price_estimate_max: product.price_estimate_max.as_ref().map(serialize_price),
            url: product.url.to_string(),
            images_hash,
            auction_start: product.auction_start.map(|dt| {
                dt.format(&time::format_description::well_known::Rfc3339)
                    .unwrap()
            }),
            auction_end: product.auction_end.map(|dt| {
                dt.format(&time::format_description::well_known::Rfc3339)
                    .unwrap()
            }),
            state: format!("{:?}", product.state).to_uppercase(),
        }
    }
}

impl ProductSnapshot {
    /// Reconstruct a snapshot from the persisted `last_scraped_*` columns in a
    /// [`ScraperCandidate`].  Used when the HTML hash matches and the field
    /// snapshot must be re-persisted unchanged (so that `last_scraped` is
    /// refreshed without touching the snapshot columns).
    pub fn from_candidate(candidate: &ScraperCandidate) -> Self {
        Self {
            price: candidate.last_scraped_price.clone(),
            price_estimate_min: candidate.last_scraped_price_estimate_min.clone(),
            price_estimate_max: candidate.last_scraped_price_estimate_max.clone(),
            url: candidate
                .last_scraped_url
                .clone()
                .unwrap_or_else(|| candidate.url.to_string()),
            images_hash: candidate
                .last_scraped_images_hash
                .clone()
                .unwrap_or_default(),
            auction_start: candidate.last_scraped_auction_start.clone(),
            auction_end: candidate.last_scraped_auction_end.clone(),
            state: candidate.last_scraped_state.clone().unwrap_or_default(),
        }
    }
}

/// Returns `true` if any tracked field in `product` differs from the persisted snapshot in
/// `candidate`.  Returns `true` (i.e. "changed") when no previous snapshot exists at all.
pub fn has_product_changed(candidate: &ScraperCandidate, product: &NormalizedProduct) -> bool {
    // If we have never successfully scraped this URL before, treat it as changed.
    if candidate.last_scraped_hash.is_none() {
        return true;
    }

    let snap = ProductSnapshot::from_normalized(product);

    snap.price != candidate.last_scraped_price
        || snap.price_estimate_min != candidate.last_scraped_price_estimate_min
        || snap.price_estimate_max != candidate.last_scraped_price_estimate_max
        || Some(snap.url.as_str()) != candidate.last_scraped_url.as_deref()
        || Some(snap.images_hash.as_str()) != candidate.last_scraped_images_hash.as_deref()
        || snap.auction_start != candidate.last_scraped_auction_start
        || snap.auction_end != candidate.last_scraped_auction_end
        || Some(snap.state.as_str()) != candidate.last_scraped_state.as_deref()
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

#[async_trait]
#[mockall::automock]
pub trait ScraperCandidateService: Send + Sync {
    async fn get_candidates(&self, limit: i64) -> Result<Vec<ScraperCandidate>, sqlx::Error>;
    async fn mark_as_scraped(
        &self,
        shop_id: &ShopId,
        url: &Url,
        hash: &str,
        snapshot: &ProductSnapshot,
    ) -> Result<(), sqlx::Error>;
    /// Touch `last_scraped` and reset failure counters without updating snapshot
    /// fields.  Called when the HTML hash matches the previous scrape — the
    /// product has not changed so only the timestamp needs refreshing.
    async fn touch_scraped(
        &self,
        shop_id: &ShopId,
        url: &Url,
        hash: &str,
    ) -> Result<(), sqlx::Error>;
    async fn set_state(
        &self,
        shop_id: &ShopId,
        url: &Url,
        state: UrlState,
    ) -> Result<(), sqlx::Error>;
    async fn mark_fetch_failure(
        &self,
        shop_id: &ShopId,
        url: &Url,
        error_kind: &str,
        status_code: Option<i32>,
        next_retry_at: OffsetDateTime,
    ) -> Result<(), sqlx::Error>;
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

pub struct ScraperCandidateServiceImpl {
    pool: PgPool,
}

impl ScraperCandidateServiceImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(sqlx::FromRow)]
struct ScraperCandidateRow {
    shop_id: uuid::Uuid,
    shop_name: String,
    shop_type: Option<String>,
    url: String,
    last_scraped_hash: Option<String>,
    last_scraped_price: Option<String>,
    last_scraped_price_estimate_min: Option<String>,
    last_scraped_price_estimate_max: Option<String>,
    last_scraped_url: Option<String>,
    last_scraped_images_hash: Option<String>,
    last_scraped_auction_start: Option<String>,
    last_scraped_auction_end: Option<String>,
    last_scraped_state: Option<String>,
}

#[async_trait]
impl ScraperCandidateService for ScraperCandidateServiceImpl {
    async fn get_candidates(&self, limit: i64) -> Result<Vec<ScraperCandidate>, sqlx::Error> {
        let rows = sqlx::query_as::<_, ScraperCandidateRow>(
            r#"
            SELECT
                su.shop_id, s.shop_name, s.shop_type, su.url,
                su.last_scraped_hash,
                su.last_scraped_price,
                su.last_scraped_price_estimate_min,
                su.last_scraped_price_estimate_max,
                su.last_scraped_url,
                su.last_scraped_images_hash,
                su.last_scraped_auction_start,
                su.last_scraped_auction_end,
                su.last_scraped_state
            FROM shop_urls su
            JOIN shops s ON s.shop_id = su.shop_id
            WHERE s.active = TRUE
              AND su.url_class = 'product'
              AND su.last_scraped_state IN ('AVAILABLE', 'UNKNOWN', 'LISTED', 'RESERVED')
              AND (su.next_retry_at IS NULL OR su.next_retry_at <= NOW())
              AND (su.last_scraped IS NULL OR su.last_scraped < NOW() - INTERVAL '1 day')
            ORDER BY su.last_scraped NULLS FIRST
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let mut candidates = Vec::new();
        for row in rows {
            let Some(url) = Url::parse(&row.url).ok() else {
                continue;
            };
            let Some(shop_type) = shop_type_from_db(row.shop_type.as_deref()) else {
                continue;
            };
            candidates.push(ScraperCandidate {
                shop_id: ShopId::from(row.shop_id),
                shop_name: row.shop_name,
                shop_type,
                url,
                last_scraped_hash: row.last_scraped_hash,
                last_scraped_price: row.last_scraped_price,
                last_scraped_price_estimate_min: row.last_scraped_price_estimate_min,
                last_scraped_price_estimate_max: row.last_scraped_price_estimate_max,
                last_scraped_url: row.last_scraped_url,
                last_scraped_images_hash: row.last_scraped_images_hash,
                last_scraped_auction_start: row.last_scraped_auction_start,
                last_scraped_auction_end: row.last_scraped_auction_end,
                last_scraped_state: row.last_scraped_state,
            });
        }

        Ok(candidates)
    }

    async fn mark_as_scraped(
        &self,
        shop_id: &ShopId,
        url: &Url,
        hash: &str,
        snapshot: &ProductSnapshot,
    ) -> Result<(), sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        let url_str = url.to_string();

        sqlx::query(
            "UPDATE shop_urls
             SET last_scraped = NOW(),
                 last_scraped_hash = $3,
                 last_scraped_price = $4,
                 last_scraped_price_estimate_min = $5,
                 last_scraped_price_estimate_max = $6,
                 last_scraped_url = $7,
                 last_scraped_images_hash = $8,
                  last_scraped_auction_start = $9,
                  last_scraped_auction_end = $10,
                  last_scraped_state = $11,
                 failure_count = 0,
                 last_error_kind = NULL,
                 last_status_code = NULL,
                 next_retry_at = NULL,
                 updated = NOW()
             WHERE shop_id = $1 AND url = $2 AND url_class = 'product'",
        )
        .bind(shop_id_uuid)
        .bind(url_str)
        .bind(hash)
        .bind(&snapshot.price)
        .bind(&snapshot.price_estimate_min)
        .bind(&snapshot.price_estimate_max)
        .bind(&snapshot.url)
        .bind(&snapshot.images_hash)
        .bind(&snapshot.auction_start)
        .bind(&snapshot.auction_end)
        .bind(&snapshot.state)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn touch_scraped(
        &self,
        shop_id: &ShopId,
        url: &Url,
        hash: &str,
    ) -> Result<(), sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        let url_str = url.to_string();

        sqlx::query(
            "UPDATE shop_urls
             SET last_scraped = NOW(),
                 last_scraped_hash = $3,
                 failure_count = 0,
                 last_error_kind = NULL,
                 last_status_code = NULL,
                 next_retry_at = NULL,
                 updated = NOW()
             WHERE shop_id = $1 AND url = $2 AND url_class = 'product'",
        )
        .bind(shop_id_uuid)
        .bind(url_str)
        .bind(hash)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn set_state(
        &self,
        shop_id: &ShopId,
        url: &Url,
        state: UrlState,
    ) -> Result<(), sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        let url_str = url.to_string();
        let state_str = state.to_string();

        sqlx::query(
            "UPDATE shop_urls
             SET last_scraped_state = $3,
                 next_retry_at = NULL,
                 updated = NOW()
             WHERE shop_id = $1 AND url = $2 AND url_class = 'product'",
        )
        .bind(shop_id_uuid)
        .bind(url_str)
        .bind(state_str)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn mark_fetch_failure(
        &self,
        shop_id: &ShopId,
        url: &Url,
        error_kind: &str,
        status_code: Option<i32>,
        next_retry_at: OffsetDateTime,
    ) -> Result<(), sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        let url_str = url.to_string();

        sqlx::query(
            "UPDATE shop_urls
             SET failure_count = failure_count + 1,
                 last_error_kind = $3,
                 last_status_code = $4,
                 next_retry_at = $5,
                 updated = NOW()
             WHERE shop_id = $1 AND url = $2 AND url_class = 'product'",
        )
        .bind(shop_id_uuid)
        .bind(url_str)
        .bind(error_kind)
        .bind(status_code)
        .bind(next_retry_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use common::{
        language::domain::Language, localized::Localized, product_state::domain::ProductState,
        shop_id::ShopId, shops_product_id::ShopsProductId,
    };
    use product::core::title::Title;
    use url::Url;

    fn base_url() -> Url {
        Url::parse("https://example.com/product/1").unwrap()
    }

    fn minimal_product() -> NormalizedProduct {
        NormalizedProduct {
            shops_product_id: ShopsProductId::from("SKU-1"),
            title: Localized::new(Language::En, Title::from("Test")),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: ProductState::Available,
            url: base_url(),
            images: vec![],
            auction_start: None,
            auction_end: None,
        }
    }

    fn candidate_with_hash(hash: &str) -> ScraperCandidate {
        ScraperCandidate {
            shop_id: ShopId::new(),
            shop_name: "Test".to_string(),
            shop_type: shop::core::shop_type::ShopType::CommercialDealer,
            url: base_url(),
            last_scraped_hash: Some(hash.to_string()),
            last_scraped_price: None,
            last_scraped_price_estimate_min: None,
            last_scraped_price_estimate_max: None,
            last_scraped_url: Some(base_url().to_string()),
            last_scraped_images_hash: None,
            last_scraped_auction_start: None,
            last_scraped_auction_end: None,
            last_scraped_state: Some("Available".to_string()),
        }
    }

    #[test]
    fn changed_when_no_previous_hash() {
        let mut candidate = candidate_with_hash("abc");
        candidate.last_scraped_hash = None;
        let product = minimal_product();
        assert!(has_product_changed(&candidate, &product));
    }

    #[test]
    fn not_changed_when_all_fields_match() {
        let product = minimal_product();
        let snap = ProductSnapshot::from_normalized(&product);
        let candidate = ScraperCandidate {
            shop_id: ShopId::new(),
            shop_name: "Test".to_string(),
            shop_type: shop::core::shop_type::ShopType::CommercialDealer,
            url: base_url(),
            last_scraped_hash: Some("somehash".to_string()),
            last_scraped_price: snap.price.clone(),
            last_scraped_price_estimate_min: snap.price_estimate_min.clone(),
            last_scraped_price_estimate_max: snap.price_estimate_max.clone(),
            last_scraped_url: Some(snap.url.clone()),
            last_scraped_images_hash: Some(snap.images_hash.clone()),
            last_scraped_auction_start: snap.auction_start.clone(),
            last_scraped_auction_end: snap.auction_end.clone(),
            last_scraped_state: Some(snap.state.clone()),
        };
        assert!(!has_product_changed(&candidate, &product));
    }

    #[test]
    fn changed_when_state_differs() {
        let product = minimal_product();
        let snap = ProductSnapshot::from_normalized(&product);
        let candidate = ScraperCandidate {
            last_scraped_state: Some("SOLD".to_string()),
            last_scraped_hash: Some("somehash".to_string()),
            last_scraped_price: snap.price.clone(),
            last_scraped_price_estimate_min: snap.price_estimate_min.clone(),
            last_scraped_price_estimate_max: snap.price_estimate_max.clone(),
            last_scraped_url: Some(snap.url.clone()),
            last_scraped_images_hash: Some(snap.images_hash.clone()),
            last_scraped_auction_start: snap.auction_start.clone(),
            last_scraped_auction_end: snap.auction_end.clone(),
            shop_id: ShopId::new(),
            shop_name: "Test".to_string(),
            shop_type: shop::core::shop_type::ShopType::CommercialDealer,
            url: base_url(),
        };
        assert!(has_product_changed(&candidate, &product));
    }
}
