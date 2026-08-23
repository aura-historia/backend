//! Service for fetching scraper candidates — product URLs that are due for re-scraping.
//!
//! A scraper candidate is a URL stored in `shop_urls` that is due for scraping by recency and
//! retry/state rules. Hash comparison is performed in-memory by the scraper after fetching HTML.
//! Each candidate carries the shop metadata (`shop_id`, `shop_name`, `shop_type`) needed to build
//! an [`UpsertProductCommand`] without an additional lookup, as well as snapshots of the last
//! successfully scraped field values used for change detection.

use async_trait::async_trait;
use shop_core::shop_id::ShopId;
use shop_core::shop_type::ShopType;
use sqlx::PgPool;
use time::OffsetDateTime;
use url::Url;

use crate::scraper::normalization::product::NormalizedProduct;
use crate::scraper::scraper_service::DEFAULT_MAX_LLM_CALLS_PER_SHOP;
use crate::service::shop_registration::shop_type_from_db;
use crate::spider::classification::url_metadata::{UrlClass, UrlState};

// ---------------------------------------------------------------------------
// ScraperCandidate
// ---------------------------------------------------------------------------

/// A product URL that is eligible for scraping, together with the shop context and the
/// last-scraped field snapshots used to detect whether the product has actually changed.
pub struct ScraperCandidate {
    pub shop_id: ShopId,
    pub shop_name: String,
    pub shop_type: ShopType,
    pub url_pattern: Option<String>,
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

/// Per-shop LLM usage snapshot for operational logging.
pub struct ShopLlmUsage {
    pub shop_id: ShopId,
    pub shop_name: String,
    pub llm_calls_count: i64,
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

        fn serialize_price(p: &money::Price) -> String {
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
    async fn get_candidates(
        &self,
        domain_limit: i64,
        urls_per_domain: i64,
        excluded_domains: &[String],
    ) -> Result<Vec<ScraperCandidate>, sqlx::Error>;
    /// Returns a random sample of product URLs for a shop (excluding the current
    /// URL) to seed first-time schema generation with additional page layouts.
    ///
    /// This query intentionally uses `ORDER BY RANDOM()` because the path is
    /// only used on schema cache misses, which are rare (typically one-time per
    /// shop unless schema rows are reset).
    async fn get_random_product_urls_for_schema_seed(
        &self,
        shop_id: &ShopId,
        exclude_url: &Url,
        limit: i64,
    ) -> Result<Vec<Url>, sqlx::Error>;
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
    async fn set_class(
        &self,
        shop_id: &ShopId,
        url: &Url,
        url_class: UrlClass,
    ) -> Result<(), sqlx::Error>;
    async fn mark_fetch_failure(
        &self,
        shop_id: &ShopId,
        url: &Url,
        error_kind: &str,
        error_message: &str,
        status_code: Option<i32>,
        next_retry_at: OffsetDateTime,
    ) -> Result<(), sqlx::Error>;

    /// Record a non-HTTP scraper failure (schema error, normalization error, etc.).
    ///
    /// Unlike [`mark_fetch_failure`] this does **not** increment `failure_count` or
    /// set a `next_retry_at` backoff — these errors are not caused by the remote
    /// server being unavailable and should not suppress future fetches.
    async fn mark_scraper_failure(
        &self,
        shop_id: &ShopId,
        url: &Url,
        error_kind: &str,
        error_message: &str,
    ) -> Result<(), sqlx::Error>;

    /// Increment per-shop LLM call counter used by schema generation flows.
    async fn increment_shop_llm_calls(
        &self,
        shop_id: &ShopId,
        delta: i64,
    ) -> Result<(), sqlx::Error>;

    /// Try to increment per-shop LLM call counter if the configured max would
    /// not be exceeded. Returns `true` when incremented, `false` when blocked
    /// by the limit.
    async fn try_increment_shop_llm_calls_with_limit(
        &self,
        shop_id: &ShopId,
        delta: i64,
        max_calls: i64,
    ) -> Result<bool, sqlx::Error>;

    /// Returns whether the per-shop LLM-call budget is already exhausted.
    async fn is_shop_llm_budget_exhausted(
        &self,
        shop_id: &ShopId,
        max_calls: i64,
    ) -> Result<bool, sqlx::Error>;

    /// Returns per-shop LLM call counts for the provided shop IDs.
    async fn get_shop_llm_usage(
        &self,
        shop_ids: Vec<ShopId>,
    ) -> Result<Vec<ShopLlmUsage>, sqlx::Error>;
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

pub struct ScraperCandidateServiceImpl {
    pool: PgPool,
    max_llm_calls_per_shop: i64,
}

impl ScraperCandidateServiceImpl {
    pub fn new(pool: PgPool) -> Self {
        Self::new_with_max_llm_calls_per_shop(pool, DEFAULT_MAX_LLM_CALLS_PER_SHOP)
    }

    pub fn new_with_max_llm_calls_per_shop(pool: PgPool, max_llm_calls_per_shop: i64) -> Self {
        Self {
            pool,
            max_llm_calls_per_shop,
        }
    }
}

#[derive(sqlx::FromRow)]
struct ScraperCandidateRow {
    shop_id: uuid::Uuid,
    shop_name: String,
    shop_type: Option<String>,
    url_pattern: Option<String>,
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

const SCRAPER_CANDIDATE_QUERY: &str = r#"
    WITH eligible_urls AS (
        SELECT
            su.shop_id, s.shop_name, s.shop_type, s.url_pattern, su.url,
            lower(substring(su.url from '^[a-z][a-z0-9+.-]*://([^/:?#]+)')) AS normalized_host,
            su.last_scraped,
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
          AND s.llm_calls_count < $3
          AND su.url_class = 'product'
          AND su.last_scraped_state IN ('AVAILABLE', 'UNKNOWN', 'LISTED', 'RESERVED')
          AND (su.next_retry_at IS NULL OR su.next_retry_at <= NOW())
          AND (su.last_scraped IS NULL OR su.last_scraped < NOW() - INTERVAL '1 day')
          AND NOT EXISTS (
              SELECT 1
              FROM crawler_reviews cr
              WHERE cr.shop_id = su.shop_id
                AND cr.artifact_type = 'PRODUCT_SCHEMA'
                AND cr.status = 'PENDING_REVIEW'
          )
          AND NOT (
              lower(substring(su.url from '^[a-z][a-z0-9+.-]*://([^/:?#]+)')) = ANY($4)
          )
    ),
    selected_domains AS (
        SELECT normalized_host
        FROM eligible_urls
        WHERE normalized_host IS NOT NULL
        GROUP BY normalized_host
        ORDER BY random()
        LIMIT $1
    ),
    ranked_urls AS (
        SELECT
            eu.*,
            row_number() OVER (
                PARTITION BY eu.normalized_host
                ORDER BY eu.last_scraped NULLS FIRST, eu.url
            ) AS domain_url_rank
        FROM eligible_urls eu
        JOIN selected_domains sd ON sd.normalized_host = eu.normalized_host
    )
    SELECT
        shop_id, shop_name, shop_type, url_pattern, url,
        last_scraped_hash,
        last_scraped_price,
        last_scraped_price_estimate_min,
        last_scraped_price_estimate_max,
        last_scraped_url,
        last_scraped_images_hash,
        last_scraped_auction_start,
        last_scraped_auction_end,
        last_scraped_state
    FROM ranked_urls
    WHERE domain_url_rank <= $2
    ORDER BY normalized_host, domain_url_rank, url
    "#;

#[async_trait]
impl ScraperCandidateService for ScraperCandidateServiceImpl {
    async fn get_candidates(
        &self,
        domain_limit: i64,
        urls_per_domain: i64,
        excluded_domains: &[String],
    ) -> Result<Vec<ScraperCandidate>, sqlx::Error> {
        let rows = sqlx::query_as::<_, ScraperCandidateRow>(SCRAPER_CANDIDATE_QUERY)
            .bind(domain_limit)
            .bind(urls_per_domain)
            .bind(self.max_llm_calls_per_shop)
            .bind(excluded_domains)
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
                url_pattern: row.url_pattern,
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

    async fn get_random_product_urls_for_schema_seed(
        &self,
        shop_id: &ShopId,
        exclude_url: &Url,
        limit: i64,
    ) -> Result<Vec<Url>, sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        let rows: Vec<(String,)> = sqlx::query_as(
            r#"
            SELECT su.url
            FROM shop_urls su
            JOIN shops s ON s.shop_id = su.shop_id
            WHERE s.active = TRUE
              AND su.shop_id = $1
              AND su.url_class = 'product'
              AND su.last_scraped_state IN ('AVAILABLE', 'UNKNOWN', 'LISTED', 'RESERVED')
              AND su.url <> $2
            -- Intentional: schema seeding runs on a rare path (typically once per
            -- shop), so ORDER BY RANDOM() keeps this simple. If rows per shop grow
            -- to millions, switch to TABLESAMPLE BERNOULLI or keyset-random.
            ORDER BY RANDOM()
            LIMIT $3
            "#,
        )
        .bind(shop_id_uuid)
        .bind(exclude_url.to_string())
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .filter_map(|(raw_url,)| Url::parse(&raw_url).ok())
            .collect())
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
                 last_error_message = NULL,
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
                 last_error_message = NULL,
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

    async fn set_class(
        &self,
        shop_id: &ShopId,
        url: &Url,
        url_class: UrlClass,
    ) -> Result<(), sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        let url_str = url.to_string();
        let url_class_str = url_class.to_string();

        sqlx::query(
            "UPDATE shop_urls
             SET url_class = $3,
                 next_retry_at = NULL,
                 updated = NOW()
             WHERE shop_id = $1 AND url = $2",
        )
        .bind(shop_id_uuid)
        .bind(url_str)
        .bind(url_class_str)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn mark_fetch_failure(
        &self,
        shop_id: &ShopId,
        url: &Url,
        error_kind: &str,
        error_message: &str,
        status_code: Option<i32>,
        next_retry_at: OffsetDateTime,
    ) -> Result<(), sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        let url_str = url.to_string();

        sqlx::query(
            "UPDATE shop_urls
             SET failure_count = failure_count + 1,
                 last_error_kind = $3,
                 last_error_message = $4,
                 last_status_code = $5,
                 next_retry_at = $6,
                 updated = NOW()
             WHERE shop_id = $1 AND url = $2 AND url_class = 'product'",
        )
        .bind(shop_id_uuid)
        .bind(url_str)
        .bind(error_kind)
        .bind(error_message)
        .bind(status_code)
        .bind(next_retry_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn mark_scraper_failure(
        &self,
        shop_id: &ShopId,
        url: &Url,
        error_kind: &str,
        error_message: &str,
    ) -> Result<(), sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        let url_str = url.to_string();

        sqlx::query(
            "UPDATE shop_urls
             SET last_error_kind = $3,
                 last_error_message = $4,
                 updated = NOW()
             WHERE shop_id = $1 AND url = $2 AND url_class = 'product'",
        )
        .bind(shop_id_uuid)
        .bind(url_str)
        .bind(error_kind)
        .bind(error_message)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn increment_shop_llm_calls(
        &self,
        shop_id: &ShopId,
        delta: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE shops
             SET llm_calls_count = llm_calls_count + $2,
                 updated = NOW()
             WHERE shop_id = $1",
        )
        .bind(uuid::Uuid::from(*shop_id))
        .bind(delta)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn try_increment_shop_llm_calls_with_limit(
        &self,
        shop_id: &ShopId,
        delta: i64,
        max_calls: i64,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE shops
             SET llm_calls_count = llm_calls_count + $2,
                 updated = NOW()
             WHERE shop_id = $1
               AND llm_calls_count + $2 <= $3",
        )
        .bind(uuid::Uuid::from(*shop_id))
        .bind(delta)
        .bind(max_calls)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn is_shop_llm_budget_exhausted(
        &self,
        shop_id: &ShopId,
        max_calls: i64,
    ) -> Result<bool, sqlx::Error> {
        let exhausted = sqlx::query_scalar::<_, bool>(
            "SELECT llm_calls_count >= $2
             FROM shops
             WHERE shop_id = $1",
        )
        .bind(uuid::Uuid::from(*shop_id))
        .bind(max_calls)
        .fetch_optional(&self.pool)
        .await?
        .unwrap_or(false);

        Ok(exhausted)
    }

    async fn get_shop_llm_usage(
        &self,
        shop_ids: Vec<ShopId>,
    ) -> Result<Vec<ShopLlmUsage>, sqlx::Error> {
        if shop_ids.is_empty() {
            return Ok(Vec::new());
        }

        let ids: Vec<uuid::Uuid> = shop_ids.into_iter().map(uuid::Uuid::from).collect();
        let rows: Vec<(uuid::Uuid, Option<String>, i64)> = sqlx::query_as(
            "SELECT shop_id, shop_name, llm_calls_count
             FROM shops
             WHERE shop_id = ANY($1::uuid[])",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(id, name, llm_calls_count)| ShopLlmUsage {
                shop_id: id.into(),
                shop_name: name.unwrap_or_else(|| id.to_string()),
                llm_calls_count,
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use localization::{Language, Localized};
    use product_core::title::Title;
    use product_core::{product_state::ProductState, shops_product_id::ShopsProductId};
    use shop_core::shop_id::ShopId;
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
            seller_name: None,
            state: ProductState::Available,
            url: base_url(),
            images: vec![],
            auction_start: None,
            auction_end: None,
            raw_attributes: Default::default(),
        }
    }

    fn candidate_with_hash(hash: &str) -> ScraperCandidate {
        ScraperCandidate {
            shop_id: ShopId::new(),
            shop_name: "Test".to_string(),
            shop_type: ShopType::CommercialDealer,
            url_pattern: None,
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
            shop_type: ShopType::CommercialDealer,
            url_pattern: None,
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
            shop_type: shop_core::shop_type::ShopType::CommercialDealer,
            url_pattern: None,
            url: base_url(),
        };
        assert!(has_product_changed(&candidate, &product));
    }

    #[test]
    fn images_hash_for_empty_list_is_sha256_of_empty_string() {
        let product = minimal_product();
        let snap = ProductSnapshot::from_normalized(&product);
        let expected = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"; // SHA256("")
        assert_eq!(snap.images_hash, expected);
    }

    #[test]
    fn images_hash_is_order_independent() {
        use product_core::product_image::ProductImage;
        use product_core::prohibited_content::ProhibitedContent;

        let images_a = vec![
            ProductImage {
                url: Url::parse("https://example.com/a.jpg").unwrap(),
                prohibited_content: ProhibitedContent::Unknown,
            },
            ProductImage {
                url: Url::parse("https://example.com/b.jpg").unwrap(),
                prohibited_content: ProhibitedContent::Unknown,
            },
        ];
        let images_b = vec![
            ProductImage {
                url: Url::parse("https://example.com/b.jpg").unwrap(),
                prohibited_content: ProhibitedContent::Unknown,
            },
            ProductImage {
                url: Url::parse("https://example.com/a.jpg").unwrap(),
                prohibited_content: ProhibitedContent::Unknown,
            },
        ];

        let mut p1 = minimal_product();
        p1.images = images_a;
        let mut p2 = minimal_product();
        p2.images = images_b;

        let h1 = ProductSnapshot::from_normalized(&p1).images_hash;
        let h2 = ProductSnapshot::from_normalized(&p2).images_hash;

        assert_eq!(h1, h2, "images_hash must be invariant to input order");
        assert_eq!(
            h1, "5c7abbc9f485b2617e096f6e54c0e090be521f588c6fc7802ddc50d5a3b627f2",
            "unexpected images_hash value"
        );
    }

    #[test]
    fn scraper_candidate_query_excludes_shops_with_pending_schema_reviews() {
        assert!(SCRAPER_CANDIDATE_QUERY.contains("NOT EXISTS"));
        assert!(SCRAPER_CANDIDATE_QUERY.contains("crawler_reviews cr"));
        assert!(SCRAPER_CANDIDATE_QUERY.contains("cr.artifact_type = 'PRODUCT_SCHEMA'"));
        assert!(SCRAPER_CANDIDATE_QUERY.contains("cr.status = 'PENDING_REVIEW'"));
    }

    #[test]
    fn scraper_candidate_query_includes_shop_url_pattern() {
        assert!(SCRAPER_CANDIDATE_QUERY.contains("s.url_pattern"));
    }
}
