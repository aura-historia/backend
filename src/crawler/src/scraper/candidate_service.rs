//! Service for fetching scraper candidates — product URLs that are due for re-scraping.
//!
//! A scraper candidate is a URL stored in `listing_source_urls` that is due for scraping by recency,
//! retry, and crawler disposition. Both active and sold URLs remain eligible so crawler evidence can
//! observe a later removal or restock. Page and schema hashes avoid needless extraction; the shared raw
//! normalization-input hash avoids needless operational raw captures.

use async_trait::async_trait;
use listing_source_core::ListingSourceId;
use money::Currency;
use sqlx::PgPool;
use time::OffsetDateTime;
use url::Url;

use crate::network::policy::DomainFailureKind;
use crate::scraper::scraper_service::DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE;
use crate::spider::classification::url_metadata::{
    CrawlerDisposition, CrawlerUrlWriteOutcome, UrlClass,
};
use crate::{CrawlerDomainId, CrawlerReviewId};

// ---------------------------------------------------------------------------
// ScraperCandidate
// ---------------------------------------------------------------------------

/// A product URL eligible for scraping with only crawler-operational state.
pub struct ScraperCandidate {
    pub listing_source_id: ListingSourceId,
    pub domain_id: CrawlerDomainId,
    pub listing_source_domain: String,
    pub listing_source_name: String,
    pub fallback_currency: Option<Currency>,
    pub url_pattern: Option<String>,
    pub url: Url,
    /// Stable raw-stream identity retained when the crawler URL moves.
    pub source_record_key: String,
    pub last_source_listing_id: Option<String>,
    pub last_scraped_hash: Option<String>,
    pub last_scraped_schema_fingerprint: Option<String>,
    pub last_captured_raw_input_sha256: Option<Vec<u8>>,
    pub domain_health: DomainHealthSnapshot,
    pub is_domain_probe: bool,
    /// The pending product-schema review the URL's last scrape waited on, if it did.
    pub schema_review_wait: Option<SchemaReviewWait>,
}

/// The pending product-schema review a URL waits on and how many consecutive scrapes have
/// waited on it. The streak drives the URL's growing retry delay while the review stays
/// pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaReviewWait {
    pub review_id: CrawlerReviewId,
    pub streak: u32,
}

impl SchemaReviewWait {
    /// The wait a scrape records when it waits on `review_id`: the streak continues only
    /// when the URL's previous scrape waited on the same review.
    pub fn after(previous: Option<SchemaReviewWait>, review_id: CrawlerReviewId) -> Self {
        let streak = match previous {
            Some(previous) if previous.review_id == review_id => previous.streak.saturating_add(1),
            _ => 1,
        };
        Self { review_id, streak }
    }
}

/// URL failure state for a page that waits on a pending product-schema review, together
/// with the wait it records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaReviewWaitRequest {
    pub url_failure: FetchFailureRequest,
    pub wait: SchemaReviewWait,
}

/// Metadata applied after durable raw capture for an HTTP redirect.
pub struct RedirectUrlMove {
    pub listing_source_id: ListingSourceId,
    pub original_url: Url,
    pub effective_url: Url,
    pub hash: String,
    pub schema_fingerprint: String,
    pub raw_input_sha256: Vec<u8>,
    pub source_listing_id: String,
    pub disposition: CrawlerDisposition,
    pub expected_last_captured_raw_input_sha256: Option<Vec<u8>>,
}

/// Durable completion state for one ordinary crawler scrape.
#[derive(Debug, Clone)]
pub struct ScrapeCompletion {
    pub listing_source_id: ListingSourceId,
    pub url: Url,
    pub hash: String,
    pub schema_fingerprint: String,
    pub raw_input_sha256: Vec<u8>,
    pub source_listing_id: String,
    pub disposition: CrawlerDisposition,
    pub expected_last_captured_raw_input_sha256: Option<Vec<u8>>,
}

/// A schema-seed URL together with the raw-input fence observed when it was
/// selected. The fence is used if the seed later produces a transport failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaSeedCandidate {
    pub url: Url,
    pub expected_last_captured_raw_input_sha256: Option<Vec<u8>>,
}

/// The scraper health columns of a crawler domain, which fence every domain
/// write. A streak without `next_scrape_at` is recorded evidence: the domain
/// is scraped normally and the evidence counts towards opening the circuit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DomainHealthSnapshot {
    pub scrape_failure_streak: i32,
    pub last_scrape_error_kind: Option<String>,
    pub next_scrape_at: Option<OffsetDateTime>,
}

/// The two independently fenced writes performed when a domain-opening fetch
/// failure is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DomainCircuitOpenOutcome {
    pub url_failure: CrawlerUrlWriteOutcome,
    pub domain_circuit: CrawlerUrlWriteOutcome,
    /// The streak known to have been persisted when the domain write applied.
    /// This is `None` when the domain write was stale.
    pub persisted_failure_streak: Option<u32>,
}

/// URL-level failure state to persist. Keeping the URL and its fence together
/// prevents a seed failure from accidentally using the primary candidate's
/// optimistic-concurrency token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchFailureRequest {
    pub listing_source_id: ListingSourceId,
    pub url: Url,
    pub error_kind: String,
    pub error_message: String,
    pub status_code: Option<i32>,
    pub next_retry_at: OffsetDateTime,
    pub expected_last_captured_raw_input_sha256: Option<Vec<u8>>,
}

/// The independently fenced URL and domain writes for a domain-opening
/// failure. The URL request identifies the actual failing URL; the domain
/// snapshot remains the candidate domain's complete optimistic-concurrency
/// fence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainCircuitOpenRequest {
    pub url_failure: FetchFailureRequest,
    pub domain_id: CrawlerDomainId,
    pub expected_domain_health: DomainHealthSnapshot,
    pub domain_error_kind: DomainFailureKind,
    pub domain_status_code: Option<i32>,
    /// End of the domain cooldown. `None` records the kind and streak as
    /// evidence while the domain stays open for scraping.
    pub next_scrape_at: Option<OffsetDateTime>,
}

/// Calculates the next domain failure streak from one candidate snapshot.
///
/// A streak is consecutive only for the same persisted domain failure kind.
pub fn next_domain_failure_streak(
    expected_domain_health: &DomainHealthSnapshot,
    domain_error_kind: DomainFailureKind,
) -> u32 {
    if expected_domain_health.last_scrape_error_kind.as_deref() == Some(domain_error_kind.as_str())
    {
        expected_domain_health
            .scrape_failure_streak
            .max(0)
            .saturating_add(1) as u32
    } else {
        1
    }
}

/// Per-ListingSource LLM usage snapshot for operational logging.
pub struct ListingSourceLlmUsage {
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: String,
    pub llm_calls_count: i64,
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
        excluded_domain_ids: &[CrawlerDomainId],
    ) -> Result<Vec<ScraperCandidate>, sqlx::Error>;
    /// Counts the URLs due for scraping, by the same eligibility rule as
    /// [`ScraperCandidateService::get_candidates`].
    async fn count_due_urls(&self) -> Result<u64, sqlx::Error>;
    /// Returns a random sample of product URLs for a ListingSource (excluding the current
    /// URL) to seed first-time schema generation with additional page layouts.
    ///
    /// This query intentionally uses `ORDER BY RANDOM()` because the path is
    /// only used on schema cache misses, which are rare (typically one-time per
    /// ListingSource unless schema rows are reset).
    async fn get_random_product_urls_for_schema_seed(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        exclude_url: &Url,
        limit: i64,
    ) -> Result<Vec<SchemaSeedCandidate>, sqlx::Error>;
    async fn mark_as_scraped(
        &self,
        completion: ScrapeCompletion,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error>;
    /// Check the redirect destination before any raw observation is queued.
    async fn redirect_destination_is_available(
        &self,
        effective_url: &Url,
    ) -> Result<bool, sqlx::Error>;
    /// Apply a redirect move only when the destination is still absent.
    async fn move_redirected_url(
        &self,
        redirect_move: RedirectUrlMove,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error>;
    /// Records a durable crawler removal capture and makes the URL eligible for later rechecks.
    async fn mark_removed(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        raw_input_sha256: &[u8],
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error>;
    /// Touch a page/schema fast-path scrape without changing the raw input or disposition.
    async fn touch_scraped(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        hash: &str,
        schema_fingerprint: &str,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error>;
    async fn set_disposition(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        disposition: CrawlerDisposition,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error>;
    async fn set_class(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        url_class: UrlClass,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error>;
    #[allow(clippy::too_many_arguments)]
    async fn mark_fetch_failure(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        error_kind: &str,
        error_message: &str,
        status_code: Option<i32>,
        next_retry_at: OffsetDateTime,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error>;

    /// Records URL failure state and attempts to open the domain circuit.
    ///
    /// The result keeps the URL and domain write outcomes separate because a
    /// current URL failure may commit even when the domain snapshot is stale.
    /// The domain update is fenced by the candidate's complete health snapshot.
    #[allow(clippy::too_many_arguments)]
    async fn mark_fetch_failure_and_open_domain_circuit(
        &self,
        request: DomainCircuitOpenRequest,
    ) -> Result<DomainCircuitOpenOutcome, sqlx::Error> {
        let url_failure = request.url_failure;
        let url_failure_outcome = self
            .mark_fetch_failure(
                &url_failure.listing_source_id,
                &url_failure.url,
                &url_failure.error_kind,
                &url_failure.error_message,
                url_failure.status_code,
                url_failure.next_retry_at,
                url_failure
                    .expected_last_captured_raw_input_sha256
                    .as_deref(),
            )
            .await?;
        Ok(DomainCircuitOpenOutcome {
            url_failure: url_failure_outcome,
            domain_circuit: CrawlerUrlWriteOutcome::NoopStale,
            persisted_failure_streak: None,
        })
    }

    /// Closes a recovering circuit if the candidate still owns the observed
    /// health snapshot. The default is a no-op for adapters without domain
    /// persistence; the Postgres implementation provides the fence.
    async fn close_domain_circuit(
        &self,
        _domain_id: &CrawlerDomainId,
        _expected_domain_health: &DomainHealthSnapshot,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        Ok(CrawlerUrlWriteOutcome::NoopStale)
    }

    /// Records URL failure state for a page that waits on a pending product-schema review,
    /// including the review it waits on and the consecutive-wait streak. The write is
    /// [`NoopStale`](CrawlerUrlWriteOutcome::NoopStale) once that review is no longer pending,
    /// so a scrape that overlapped the verdict on the review cannot hold back a page the verdict
    /// released.
    async fn mark_waiting_on_schema_review(
        &self,
        request: SchemaReviewWaitRequest,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error>;

    /// Record a non-HTTP scraper failure (schema error, implausible fresh extraction, etc.).
    ///
    /// Unlike [`mark_fetch_failure`] this does **not** increment `failure_count` or
    /// set a `next_retry_at` backoff — these errors are not caused by the remote
    /// server being unavailable and should not suppress future fetches.
    async fn mark_scraper_failure(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        error_kind: &str,
        error_message: &str,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error>;

    /// Records a scrape that evaluated the page but has nothing to capture, such as a page
    /// that leaves a required value empty. The URL counts as scraped, so it returns at its
    /// normal scrape interval instead of after a retry cooldown, and keeps `outcome_kind` and
    /// `outcome_message` as its last error until a later scrape completes. Page, schema and
    /// raw-input hashes stay those of the last completed scrape, so the next scrape
    /// evaluates the page again.
    async fn mark_scraped_without_capture(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        outcome_kind: &str,
        outcome_message: &str,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error>;

    /// Increment per-ListingSource LLM call counter used by schema generation flows.
    async fn increment_listing_source_llm_calls(
        &self,
        listing_source_id: &ListingSourceId,
        delta: i64,
    ) -> Result<(), sqlx::Error>;

    /// Try to increment per-ListingSource LLM call counter if the configured max would
    /// not be exceeded. Returns `true` when incremented, `false` when blocked
    /// by the limit.
    async fn try_increment_listing_source_llm_calls_with_limit(
        &self,
        listing_source_id: &ListingSourceId,
        delta: i64,
        max_calls: i64,
    ) -> Result<bool, sqlx::Error>;

    /// Returns whether the per-ListingSource LLM-call budget is already exhausted.
    async fn is_listing_source_llm_budget_exhausted(
        &self,
        listing_source_id: &ListingSourceId,
        max_calls: i64,
    ) -> Result<bool, sqlx::Error>;

    /// Returns per-ListingSource LLM call counts for the provided ListingSource IDs.
    async fn get_listing_source_llm_usage(
        &self,
        listing_source_ids: Vec<ListingSourceId>,
    ) -> Result<Vec<ListingSourceLlmUsage>, sqlx::Error>;
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

pub struct ScraperCandidateServiceImpl {
    pool: PgPool,
    max_llm_calls_per_listing_source: i64,
}

impl ScraperCandidateServiceImpl {
    pub fn new(pool: PgPool) -> Self {
        Self::new_with_max_llm_calls_per_listing_source(
            pool,
            DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
        )
    }

    pub fn new_with_max_llm_calls_per_listing_source(
        pool: PgPool,
        max_llm_calls_per_listing_source: i64,
    ) -> Self {
        Self {
            pool,
            max_llm_calls_per_listing_source,
        }
    }

    /// Writes URL failure state, fenced on the raw-input hash the candidate observed. Only a
    /// page waiting on a schema review records a review wait, and only while that product-schema
    /// review of the URL's ListingSource is still pending; every other failure clears the wait.
    /// Candidate selection reads a wait only while `last_error_kind` is still
    /// `PendingSchemaReview`, so writes that leave the columns cannot extend a streak either.
    ///
    /// The pending check share-locks the review row before the URL row is written. Approval and
    /// rejection lock the review row first and release the waiting pages last, so a wait written
    /// while either verdict commits waits for it and then finds the review closed, instead of
    /// passing a check that read the review before the verdict committed and overwriting the page
    /// the verdict just released.
    async fn update_url_failure(
        &self,
        failure: UrlFailureUpdate<'_>,
        schema_review_wait: Option<SchemaReviewWait>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        let listing_source_id_uuid: uuid::Uuid = (*failure.listing_source_id).into();
        let streak = schema_review_wait.map_or(Ok(0), |wait| i32::try_from(wait.streak));
        let streak = streak.map_err(|error| sqlx::Error::Encode(Box::new(error)))?;

        let result = sqlx::query(
            "UPDATE listing_source_urls
             SET failure_count = failure_count + 1,
                 last_error_kind = $3,
                 last_error_message = $4,
                 last_status_code = $5,
                 next_retry_at = $6,
                 schema_review_waited_on = $8,
                 schema_review_wait_streak = $9,
                 updated = NOW()
             WHERE listing_source_id = $1
               AND url = $2
               AND url_class = 'product'
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
               AND last_captured_raw_input_sha256 IS NOT DISTINCT FROM $7::bytea
               AND ($8::uuid IS NULL OR EXISTS (
                   SELECT 1
                   FROM crawler_reviews cr
                   WHERE cr.review_id = $8
                     AND cr.listing_source_id = $1
                     AND cr.artifact_type = 'PRODUCT_SCHEMA'
                     AND cr.status = 'PENDING_REVIEW'
                   FOR SHARE
               ))",
        )
        .bind(listing_source_id_uuid)
        .bind(failure.url.to_string())
        .bind(failure.error_kind)
        .bind(failure.error_message)
        .bind(failure.status_code)
        .bind(failure.next_retry_at)
        .bind(failure.expected_last_captured_raw_input_sha256)
        .bind(schema_review_wait.map(|wait| *wait.review_id.as_uuid()))
        .bind(streak)
        .execute(&self.pool)
        .await?;

        Ok(if result.rows_affected() == 1 {
            CrawlerUrlWriteOutcome::Applied
        } else {
            CrawlerUrlWriteOutcome::NoopStale
        })
    }
}

/// The URL failure columns one fenced failure write sets.
struct UrlFailureUpdate<'a> {
    listing_source_id: &'a ListingSourceId,
    url: &'a Url,
    error_kind: &'a str,
    error_message: &'a str,
    status_code: Option<i32>,
    next_retry_at: OffsetDateTime,
    expected_last_captured_raw_input_sha256: Option<&'a [u8]>,
}

/// Decodes a persisted review wait, failing closed when the review id and streak disagree.
fn schema_review_wait_from_row(
    review_id: Option<uuid::Uuid>,
    streak: i32,
) -> Result<Option<SchemaReviewWait>, sqlx::Error> {
    let invalid = |message: &'static str| sqlx::Error::Decode(message.into());
    match (review_id, u32::try_from(streak)) {
        (None, Ok(0)) => Ok(None),
        (Some(review_id), Ok(streak)) if streak > 0 => Ok(Some(SchemaReviewWait {
            review_id: CrawlerReviewId::try_from(review_id)
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
            streak,
        })),
        _ => Err(invalid("persisted crawler schema review wait is invalid")),
    }
}

#[derive(sqlx::FromRow)]
struct ScraperCandidateRow {
    listing_source_id: uuid::Uuid,
    domain_id: uuid::Uuid,
    listing_source_domain: String,
    listing_source_name: String,
    fallback_currency: Option<String>,

    url_pattern: Option<String>,
    url: String,
    source_record_key: String,
    last_scraped_hash: Option<String>,
    last_scraped_schema_fingerprint: Option<String>,
    last_captured_raw_input_sha256: Option<Vec<u8>>,
    scrape_failure_streak: i32,
    last_scrape_error_kind: Option<String>,
    next_scrape_at: Option<OffsetDateTime>,
    is_domain_probe: bool,
    last_source_listing_id: Option<String>,
    schema_review_waited_on: Option<uuid::Uuid>,
    schema_review_wait_streak: i32,
}

/// The URLs due for scraping, shared by every query that selects or counts them so their
/// definitions cannot drift. Binds `$1`, the per-ListingSource LLM call cap, and `$2`, the
/// domain IDs to leave out.
macro_rules! eligible_scraper_urls_cte {
    () => {
        r#"
    WITH eligible_urls AS (
        SELECT
            su.listing_source_id, sd.domain_id, sd.listing_source_domain,
            s.listing_source_name, s.fallback_currency, sd.url_pattern, su.url,
            su.raw_source_record_key AS source_record_key,
            su.last_scraped,
            su.last_scraped_hash,
            su.last_scraped_schema_fingerprint,
            su.last_captured_raw_input_sha256,
            sd.scrape_failure_streak,
            sd.last_scrape_error_kind,
            sd.next_scrape_at,
            (sd.scrape_failure_streak > 0
                AND sd.next_scrape_at IS NOT NULL
                AND sd.next_scrape_at <= NOW()) AS is_domain_probe
            , su.last_source_listing_id
            , su.last_error_kind = 'PendingSchemaReview' AS waits_on_schema_review
            , CASE WHEN su.last_error_kind = 'PendingSchemaReview'
                   THEN su.schema_review_waited_on END AS schema_review_waited_on
            , CASE WHEN su.last_error_kind = 'PendingSchemaReview'
                   THEN su.schema_review_wait_streak ELSE 0 END AS schema_review_wait_streak
        FROM listing_source_urls su
        JOIN listing_sources s ON s.listing_source_id = su.listing_source_id
        JOIN listing_source_domains sd
          ON sd.listing_source_id = su.listing_source_id AND sd.domain_id = su.domain_id
        WHERE s.crawl_enabled = TRUE
          AND s.llm_calls_count < $1
          AND su.url_class = 'product'
          AND su.crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
          AND (su.next_retry_at IS NULL OR su.next_retry_at <= NOW())
          AND (su.last_scraped IS NULL OR su.last_scraped < NOW() - INTERVAL '1 day')
          AND (sd.scrape_failure_streak = 0
               OR sd.next_scrape_at IS NULL
               OR sd.next_scrape_at <= NOW())
          AND (
              EXISTS (
                  SELECT 1
                  FROM listing_source_product_schemas ps
                  WHERE ps.listing_source_id = su.listing_source_id
                    AND (jsonb_typeof(ps.product_schema) <> 'array'
                         OR jsonb_array_length(ps.product_schema) > 0)
              )
              OR NOT EXISTS (
                  SELECT 1
                  FROM crawler_reviews cr
                  WHERE cr.listing_source_id = su.listing_source_id
                    AND cr.artifact_type = 'PRODUCT_SCHEMA'
                    AND cr.status = 'PENDING_REVIEW'
              )
          )
          AND NOT (sd.domain_id = ANY($2))
    )"#
    };
}

/// Binds `$3`, the number of domains, and `$4`, the URLs per domain, after the
/// [`eligible_scraper_urls_cte`] parameters.
const SCRAPER_CANDIDATE_QUERY: &str = concat!(
    eligible_scraper_urls_cte!(),
    r#",
    selected_domains AS (
        SELECT domain_id
        FROM eligible_urls
        GROUP BY domain_id
        ORDER BY random()
        LIMIT $3
    ),
    ranked_urls AS (
        SELECT
            eu.*,
            row_number() OVER (
                PARTITION BY eu.domain_id
                ORDER BY eu.waits_on_schema_review IS TRUE, eu.last_scraped NULLS FIRST, eu.url
            ) AS domain_url_rank
        FROM eligible_urls eu
        JOIN selected_domains sd ON sd.domain_id = eu.domain_id
    )
    SELECT
        listing_source_id, domain_id, listing_source_domain, listing_source_name,
        fallback_currency, url_pattern, url,
        source_record_key,
        last_scraped_hash,
        last_scraped_schema_fingerprint,
        last_captured_raw_input_sha256,
        scrape_failure_streak, last_scrape_error_kind, next_scrape_at, is_domain_probe,
        last_source_listing_id, schema_review_waited_on, schema_review_wait_streak
    FROM ranked_urls
    WHERE (is_domain_probe AND domain_url_rank = 1)
       OR (NOT is_domain_probe AND domain_url_rank <= $4)
    ORDER BY domain_id, domain_url_rank, url
    "#
);

const DUE_SCRAPER_URL_COUNT_QUERY: &str = concat!(
    eligible_scraper_urls_cte!(),
    "\n    SELECT COUNT(*) FROM eligible_urls"
);

#[async_trait]
impl ScraperCandidateService for ScraperCandidateServiceImpl {
    async fn get_candidates(
        &self,
        domain_limit: i64,
        urls_per_domain: i64,
        excluded_domain_ids: &[CrawlerDomainId],
    ) -> Result<Vec<ScraperCandidate>, sqlx::Error> {
        let excluded_domain_uuids: Vec<uuid::Uuid> = excluded_domain_ids
            .iter()
            .map(|domain_id| *domain_id.as_uuid())
            .collect();
        let rows = sqlx::query_as::<_, ScraperCandidateRow>(SCRAPER_CANDIDATE_QUERY)
            .bind(self.max_llm_calls_per_listing_source)
            .bind(excluded_domain_uuids)
            .bind(domain_limit)
            .bind(urls_per_domain)
            .fetch_all(&self.pool)
            .await?;

        let mut candidates = Vec::new();
        for row in rows {
            let Some(url) = Url::parse(&row.url).ok() else {
                continue;
            };
            let fallback_currency = row
                .fallback_currency
                .as_deref()
                .map(|value| {
                    Currency::from_code(value).ok_or_else(|| {
                        sqlx::Error::Decode(Box::new(std::io::Error::other(
                            "persisted crawler fallback currency is invalid",
                        )))
                    })
                })
                .transpose()?;
            let schema_review_wait = schema_review_wait_from_row(
                row.schema_review_waited_on,
                row.schema_review_wait_streak,
            )?;
            candidates.push(ScraperCandidate {
                listing_source_id: ListingSourceId::try_from(row.listing_source_id)
                    .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
                domain_id: CrawlerDomainId::try_from(row.domain_id)
                    .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
                listing_source_domain: row.listing_source_domain,
                listing_source_name: row.listing_source_name,
                fallback_currency,
                url_pattern: row.url_pattern,
                url,
                source_record_key: row.source_record_key,
                last_scraped_hash: row.last_scraped_hash,
                last_scraped_schema_fingerprint: row.last_scraped_schema_fingerprint,
                last_captured_raw_input_sha256: row.last_captured_raw_input_sha256,
                domain_health: DomainHealthSnapshot {
                    scrape_failure_streak: row.scrape_failure_streak,
                    last_scrape_error_kind: row.last_scrape_error_kind,
                    next_scrape_at: row.next_scrape_at,
                },
                is_domain_probe: row.is_domain_probe,
                last_source_listing_id: row.last_source_listing_id,
                schema_review_wait,
            });
        }

        Ok(candidates)
    }

    async fn count_due_urls(&self) -> Result<u64, sqlx::Error> {
        let count: i64 = sqlx::query_scalar(DUE_SCRAPER_URL_COUNT_QUERY)
            .bind(self.max_llm_calls_per_listing_source)
            .bind(Vec::<uuid::Uuid>::new())
            .fetch_one(&self.pool)
            .await?;
        u64::try_from(count).map_err(|error| sqlx::Error::Decode(Box::new(error)))
    }

    async fn get_random_product_urls_for_schema_seed(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        exclude_url: &Url,
        limit: i64,
    ) -> Result<Vec<SchemaSeedCandidate>, sqlx::Error> {
        let listing_source_id_uuid: uuid::Uuid = (*listing_source_id).into();
        let domain_id_uuid = (*domain_id).into_uuid();
        let rows: Vec<(String, Option<Vec<u8>>)> = sqlx::query_as(
            r#"
            SELECT su.url, su.last_captured_raw_input_sha256
            FROM listing_source_urls su
            JOIN listing_sources s ON s.listing_source_id = su.listing_source_id
            JOIN listing_source_domains sd
              ON sd.listing_source_id = su.listing_source_id
             AND sd.domain_id = su.domain_id
            WHERE s.crawl_enabled = TRUE
              AND su.listing_source_id = $1
              AND su.domain_id = $2
              AND su.url_class = 'product'
              AND su.crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
              AND su.url <> $3
              AND (su.next_retry_at IS NULL OR su.next_retry_at <= NOW())
              AND su.failure_count = 0
              AND (sd.scrape_failure_streak = 0
                   OR sd.next_scrape_at IS NULL
                   OR sd.next_scrape_at <= NOW())
            -- Intentional: schema seeding runs on a rare path (typically once per
            -- ListingSource), so ORDER BY RANDOM() keeps this simple. If rows per ListingSource grow
            -- to millions, switch to TABLESAMPLE BERNOULLI or keyset-random.
            ORDER BY RANDOM()
            LIMIT $4
            "#,
        )
        .bind(listing_source_id_uuid)
        .bind(domain_id_uuid)
        .bind(exclude_url.to_string())
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .filter_map(|(raw_url, expected_last_captured_raw_input_sha256)| {
                Url::parse(&raw_url).ok().map(|url| SchemaSeedCandidate {
                    url,
                    expected_last_captured_raw_input_sha256,
                })
            })
            .collect())
    }

    async fn mark_as_scraped(
        &self,
        completion: ScrapeCompletion,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        let ScrapeCompletion {
            listing_source_id,
            url,
            hash,
            schema_fingerprint,
            raw_input_sha256,
            source_listing_id,
            disposition,
            expected_last_captured_raw_input_sha256,
        } = completion;
        let listing_source_id_uuid: uuid::Uuid = listing_source_id.into();
        let url_str = url.to_string();

        let result = sqlx::query(
            "UPDATE listing_source_urls
             SET last_scraped = NOW(),
                 last_scraped_hash = $3,
                 last_scraped_schema_fingerprint = $4,
                 last_captured_raw_input_sha256 = $5,
                 last_source_listing_id = $6,
                 crawler_disposition = $7,
                 failure_count = 0,
                 last_error_kind = NULL,
                 last_error_message = NULL,
                 last_status_code = NULL,
                 next_retry_at = NULL,
                 schema_review_waited_on = NULL,
                 schema_review_wait_streak = 0,
                 updated = NOW()
             WHERE listing_source_id = $1
               AND url = $2
               AND url_class = 'product'
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
               AND last_captured_raw_input_sha256 IS NOT DISTINCT FROM $8::bytea",
        )
        .bind(listing_source_id_uuid)
        .bind(url_str)
        .bind(&hash)
        .bind(&schema_fingerprint)
        .bind(&raw_input_sha256)
        .bind(&source_listing_id)
        .bind(disposition.as_str())
        .bind(expected_last_captured_raw_input_sha256.as_deref())
        .execute(&self.pool)
        .await?;

        Ok(if result.rows_affected() == 1 {
            CrawlerUrlWriteOutcome::Applied
        } else {
            CrawlerUrlWriteOutcome::NoopStale
        })
    }

    async fn redirect_destination_is_available(
        &self,
        effective_url: &Url,
    ) -> Result<bool, sqlx::Error> {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM listing_source_urls WHERE url = $1)")
                .bind(effective_url.to_string())
                .fetch_one(&self.pool)
                .await?;
        Ok(!exists)
    }

    async fn move_redirected_url(
        &self,
        redirect_move: RedirectUrlMove,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        let RedirectUrlMove {
            listing_source_id,
            original_url,
            effective_url,
            hash,
            schema_fingerprint,
            raw_input_sha256,
            source_listing_id,
            disposition,
            expected_last_captured_raw_input_sha256,
        } = redirect_move;
        if original_url == effective_url {
            return Err(sqlx::Error::Protocol(
                "redirected crawler URL move requires distinct URLs".to_owned(),
            ));
        }

        let listing_source_id_uuid: uuid::Uuid = listing_source_id.into();
        let mut transaction = self.pool.begin().await?;
        let source_identity: Option<Option<String>> = sqlx::query_scalar(
            "SELECT last_source_listing_id
             FROM listing_source_urls
             WHERE listing_source_id = $1
               AND url = $2
               AND url_class = 'product'
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
               AND last_captured_raw_input_sha256 IS NOT DISTINCT FROM $3::bytea
             FOR UPDATE",
        )
        .bind(listing_source_id_uuid)
        .bind(original_url.to_string())
        .bind(expected_last_captured_raw_input_sha256.as_deref())
        .fetch_optional(&mut *transaction)
        .await?;

        let Some(source_identity) = source_identity else {
            return Ok(CrawlerUrlWriteOutcome::NoopStale);
        };
        if source_identity
            .as_deref()
            .is_some_and(|identity| identity != source_listing_id)
        {
            return Err(sqlx::Error::Protocol(
                "redirect source identity conflicts with the established crawler identity"
                    .to_owned(),
            ));
        }

        let destination_exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM listing_source_urls WHERE url = $1)")
                .bind(effective_url.to_string())
                .fetch_one(&mut *transaction)
                .await?;
        if destination_exists {
            return Err(sqlx::Error::Protocol(
                "redirect destination already known; reconciliation deferred to #1899".to_owned(),
            ));
        }

        let moved = sqlx::query(
            "UPDATE listing_source_urls
             SET url = $3,
                 last_source_listing_id = $4,
                 last_scraped = NOW(),
                 last_scraped_hash = $5,
                 last_scraped_schema_fingerprint = $6,
                 last_captured_raw_input_sha256 = $7,
                 crawler_disposition = $8,
                 failure_count = 0,
                 last_error_kind = NULL,
                 last_error_message = NULL,
                 last_status_code = NULL,
                 next_retry_at = NULL,
                 updated = NOW()
             WHERE listing_source_id = $1
               AND url = $2
               AND last_captured_raw_input_sha256 IS NOT DISTINCT FROM $9::bytea",
        )
        .bind(listing_source_id_uuid)
        .bind(original_url.to_string())
        .bind(effective_url.to_string())
        .bind(&source_listing_id)
        .bind(&hash)
        .bind(&schema_fingerprint)
        .bind(&raw_input_sha256)
        .bind(disposition.as_str())
        .bind(expected_last_captured_raw_input_sha256.as_deref())
        .execute(&mut *transaction)
        .await?;
        if moved.rows_affected() != 1 {
            return Ok(CrawlerUrlWriteOutcome::NoopStale);
        }
        transaction.commit().await?;
        Ok(CrawlerUrlWriteOutcome::Applied)
    }

    async fn mark_removed(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        raw_input_sha256: &[u8],
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        let listing_source_id_uuid: uuid::Uuid = (*listing_source_id).into();
        let result = sqlx::query(
            "UPDATE listing_source_urls
             SET last_scraped = NOW(),
                 last_captured_raw_input_sha256 = $3,
                 crawler_disposition = 'ACTIVE',
                 failure_count = 0,
                 last_error_kind = NULL,
                 last_error_message = NULL,
                 last_status_code = NULL,
                 next_retry_at = NULL,
                 updated = NOW()
             WHERE listing_source_id = $1
               AND url = $2
               AND url_class = 'product'
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
               AND last_captured_raw_input_sha256 IS NOT DISTINCT FROM $4::bytea",
        )
        .bind(listing_source_id_uuid)
        .bind(url.to_string())
        .bind(raw_input_sha256)
        .bind(expected_last_captured_raw_input_sha256)
        .execute(&self.pool)
        .await?;

        Ok(if result.rows_affected() == 1 {
            CrawlerUrlWriteOutcome::Applied
        } else {
            CrawlerUrlWriteOutcome::NoopStale
        })
    }

    async fn touch_scraped(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        hash: &str,
        schema_fingerprint: &str,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        let listing_source_id_uuid: uuid::Uuid = (*listing_source_id).into();
        let url_str = url.to_string();

        let result = sqlx::query(
            "UPDATE listing_source_urls
             SET last_scraped = NOW(),
                 last_scraped_hash = $3,
                 last_scraped_schema_fingerprint = $4,
                 failure_count = 0,
                 last_error_kind = NULL,
                 last_error_message = NULL,
                 last_status_code = NULL,
                 next_retry_at = NULL,
                 updated = NOW()
             WHERE listing_source_id = $1
               AND url = $2
               AND url_class = 'product'
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
               AND last_captured_raw_input_sha256 IS NOT DISTINCT FROM $5::bytea",
        )
        .bind(listing_source_id_uuid)
        .bind(url_str)
        .bind(hash)
        .bind(schema_fingerprint)
        .bind(expected_last_captured_raw_input_sha256)
        .execute(&self.pool)
        .await?;

        Ok(if result.rows_affected() == 1 {
            CrawlerUrlWriteOutcome::Applied
        } else {
            CrawlerUrlWriteOutcome::NoopStale
        })
    }

    async fn set_disposition(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        disposition: CrawlerDisposition,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        let listing_source_id_uuid: uuid::Uuid = (*listing_source_id).into();
        let url_str = url.to_string();
        let result = sqlx::query(
            "UPDATE listing_source_urls
             SET crawler_disposition = $3,
                 next_retry_at = NULL,
                 updated = NOW()
             WHERE listing_source_id = $1
               AND url = $2
               AND url_class = 'product'
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
               AND last_captured_raw_input_sha256 IS NOT DISTINCT FROM $4::bytea",
        )
        .bind(listing_source_id_uuid)
        .bind(url_str)
        .bind(disposition.as_str())
        .bind(expected_last_captured_raw_input_sha256)
        .execute(&self.pool)
        .await?;

        Ok(if result.rows_affected() == 1 {
            CrawlerUrlWriteOutcome::Applied
        } else {
            CrawlerUrlWriteOutcome::NoopStale
        })
    }

    async fn set_class(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        url_class: UrlClass,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        let listing_source_id_uuid: uuid::Uuid = (*listing_source_id).into();
        let url_str = url.to_string();
        let url_class_str = url_class.to_string();

        let result = sqlx::query(
            "UPDATE listing_source_urls
             SET url_class = $3,
                 next_retry_at = NULL,
                 updated = NOW()
             WHERE listing_source_id = $1
               AND url = $2
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
               AND last_captured_raw_input_sha256 IS NOT DISTINCT FROM $4::bytea",
        )
        .bind(listing_source_id_uuid)
        .bind(url_str)
        .bind(url_class_str)
        .bind(expected_last_captured_raw_input_sha256)
        .execute(&self.pool)
        .await?;

        Ok(if result.rows_affected() == 1 {
            CrawlerUrlWriteOutcome::Applied
        } else {
            CrawlerUrlWriteOutcome::NoopStale
        })
    }

    async fn mark_fetch_failure(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        error_kind: &str,
        error_message: &str,
        status_code: Option<i32>,
        next_retry_at: OffsetDateTime,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        self.update_url_failure(
            UrlFailureUpdate {
                listing_source_id,
                url,
                error_kind,
                error_message,
                status_code,
                next_retry_at,
                expected_last_captured_raw_input_sha256,
            },
            None,
        )
        .await
    }

    async fn mark_waiting_on_schema_review(
        &self,
        request: SchemaReviewWaitRequest,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        let SchemaReviewWaitRequest { url_failure, wait } = request;
        self.update_url_failure(
            UrlFailureUpdate {
                listing_source_id: &url_failure.listing_source_id,
                url: &url_failure.url,
                error_kind: &url_failure.error_kind,
                error_message: &url_failure.error_message,
                status_code: url_failure.status_code,
                next_retry_at: url_failure.next_retry_at,
                expected_last_captured_raw_input_sha256: url_failure
                    .expected_last_captured_raw_input_sha256
                    .as_deref(),
            },
            Some(wait),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn mark_fetch_failure_and_open_domain_circuit(
        &self,
        request: DomainCircuitOpenRequest,
    ) -> Result<DomainCircuitOpenOutcome, sqlx::Error> {
        let url_failure = request.url_failure;
        let url_failure_outcome = self
            .mark_fetch_failure(
                &url_failure.listing_source_id,
                &url_failure.url,
                &url_failure.error_kind,
                &url_failure.error_message,
                url_failure.status_code,
                url_failure.next_retry_at,
                url_failure
                    .expected_last_captured_raw_input_sha256
                    .as_deref(),
            )
            .await?;

        let next_streak =
            next_domain_failure_streak(&request.expected_domain_health, request.domain_error_kind);
        let domain_result = sqlx::query(
            "UPDATE listing_source_domains
             SET scrape_failure_streak = $2,
                 last_scrape_error_kind = $3,
                 last_scrape_status_code = $4,
                 next_scrape_at = $5
             WHERE domain_id = $1
               AND scrape_failure_streak = $6
               AND next_scrape_at IS NOT DISTINCT FROM $7
               AND last_scrape_error_kind IS NOT DISTINCT FROM $8",
        )
        .bind(request.domain_id.as_uuid())
        .bind(i32::try_from(next_streak).expect("domain failure streak must fit persisted INT"))
        .bind(request.domain_error_kind.as_str())
        .bind(request.domain_status_code)
        .bind(request.next_scrape_at)
        .bind(request.expected_domain_health.scrape_failure_streak)
        .bind(request.expected_domain_health.next_scrape_at)
        .bind(
            request
                .expected_domain_health
                .last_scrape_error_kind
                .as_deref(),
        )
        .execute(&self.pool)
        .await?;

        let domain_circuit = if domain_result.rows_affected() == 1 {
            CrawlerUrlWriteOutcome::Applied
        } else {
            CrawlerUrlWriteOutcome::NoopStale
        };

        if domain_circuit == CrawlerUrlWriteOutcome::NoopStale {
            tracing::debug!(
                domain_id = %request.domain_id,
                "Skipped stale scraper domain circuit opening"
            );
        }
        Ok(DomainCircuitOpenOutcome {
            url_failure: url_failure_outcome,
            domain_circuit,
            persisted_failure_streak: (domain_circuit == CrawlerUrlWriteOutcome::Applied)
                .then_some(next_streak),
        })
    }

    async fn close_domain_circuit(
        &self,
        domain_id: &CrawlerDomainId,
        expected_domain_health: &DomainHealthSnapshot,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        if expected_domain_health.scrape_failure_streak <= 0 {
            return Ok(CrawlerUrlWriteOutcome::Applied);
        }

        let result = sqlx::query(
            "UPDATE listing_source_domains
             SET scrape_failure_streak = 0,
                 last_scrape_error_kind = NULL,
                 last_scrape_status_code = NULL,
                 next_scrape_at = NULL
             WHERE domain_id = $1
               AND scrape_failure_streak = $2
               AND next_scrape_at IS NOT DISTINCT FROM $3
               AND last_scrape_error_kind IS NOT DISTINCT FROM $4",
        )
        .bind(domain_id.as_uuid())
        .bind(expected_domain_health.scrape_failure_streak)
        .bind(expected_domain_health.next_scrape_at)
        .bind(expected_domain_health.last_scrape_error_kind.as_deref())
        .execute(&self.pool)
        .await?;

        Ok(if result.rows_affected() == 1 {
            CrawlerUrlWriteOutcome::Applied
        } else {
            CrawlerUrlWriteOutcome::NoopStale
        })
    }

    async fn mark_scraper_failure(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        error_kind: &str,
        error_message: &str,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        let listing_source_id_uuid: uuid::Uuid = (*listing_source_id).into();
        let url_str = url.to_string();

        let result = sqlx::query(
            "UPDATE listing_source_urls
             SET last_error_kind = $3,
                 last_error_message = $4,
                 updated = NOW()
             WHERE listing_source_id = $1
               AND url = $2
               AND url_class = 'product'
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
               AND last_captured_raw_input_sha256 IS NOT DISTINCT FROM $5::bytea",
        )
        .bind(listing_source_id_uuid)
        .bind(url_str)
        .bind(error_kind)
        .bind(error_message)
        .bind(expected_last_captured_raw_input_sha256)
        .execute(&self.pool)
        .await?;

        Ok(if result.rows_affected() == 1 {
            CrawlerUrlWriteOutcome::Applied
        } else {
            CrawlerUrlWriteOutcome::NoopStale
        })
    }

    async fn mark_scraped_without_capture(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        outcome_kind: &str,
        outcome_message: &str,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE listing_source_urls
             SET last_scraped = NOW(),
                 failure_count = 0,
                 last_error_kind = $3,
                 last_error_message = $4,
                 last_status_code = NULL,
                 next_retry_at = NULL,
                 updated = NOW()
             WHERE listing_source_id = $1
               AND url = $2
               AND url_class = 'product'
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
               AND last_captured_raw_input_sha256 IS NOT DISTINCT FROM $5::bytea",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(url.as_str())
        .bind(outcome_kind)
        .bind(outcome_message)
        .bind(expected_last_captured_raw_input_sha256)
        .execute(&self.pool)
        .await?;

        Ok(if result.rows_affected() == 1 {
            CrawlerUrlWriteOutcome::Applied
        } else {
            CrawlerUrlWriteOutcome::NoopStale
        })
    }

    async fn increment_listing_source_llm_calls(
        &self,
        listing_source_id: &ListingSourceId,
        delta: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE listing_sources
             SET llm_calls_count = llm_calls_count + $2,
                 updated = NOW()
             WHERE listing_source_id = $1",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(delta)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn try_increment_listing_source_llm_calls_with_limit(
        &self,
        listing_source_id: &ListingSourceId,
        delta: i64,
        max_calls: i64,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE listing_sources
             SET llm_calls_count = llm_calls_count + $2,
                 updated = NOW()
             WHERE listing_source_id = $1
               AND llm_calls_count + $2 <= $3",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(delta)
        .bind(max_calls)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn is_listing_source_llm_budget_exhausted(
        &self,
        listing_source_id: &ListingSourceId,
        max_calls: i64,
    ) -> Result<bool, sqlx::Error> {
        let exhausted = sqlx::query_scalar::<_, bool>(
            "SELECT llm_calls_count >= $2
             FROM listing_sources
             WHERE listing_source_id = $1",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(max_calls)
        .fetch_optional(&self.pool)
        .await?
        .unwrap_or(false);

        Ok(exhausted)
    }

    async fn get_listing_source_llm_usage(
        &self,
        listing_source_ids: Vec<ListingSourceId>,
    ) -> Result<Vec<ListingSourceLlmUsage>, sqlx::Error> {
        if listing_source_ids.is_empty() {
            return Ok(Vec::new());
        }

        let ids: Vec<uuid::Uuid> = listing_source_ids
            .into_iter()
            .map(uuid::Uuid::from)
            .collect();
        let rows: Vec<(uuid::Uuid, Option<String>, i64)> = sqlx::query_as(
            "SELECT listing_source_id, listing_source_name, llm_calls_count
             FROM listing_sources
             WHERE listing_source_id = ANY($1::uuid[])",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|(id, name, llm_calls_count)| {
                let listing_source_id = ListingSourceId::try_from(id).map_err(|source| {
                    sqlx::Error::Decode(Box::new(PersistedListingSourceIdError {
                        value: id,
                        source,
                    }))
                })?;
                Ok(ListingSourceLlmUsage {
                    listing_source_id,
                    listing_source_name: name.unwrap_or_else(|| listing_source_id.to_string()),
                    llm_calls_count,
                })
            })
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid persisted ListingSourceId UUID `{value}`")]
struct PersistedListingSourceIdError {
    value: uuid::Uuid,
    #[source]
    source: domain_primitives::object_id::ObjectIdError,
}

#[cfg(test)]
mod candidate_query_tests {
    use super::{
        DomainFailureKind, DomainHealthSnapshot, SCRAPER_CANDIDATE_QUERY, SchemaReviewWait,
        next_domain_failure_streak, schema_review_wait_from_row,
    };
    use crate::CrawlerReviewId;

    #[test]
    fn should_select_active_and_sold_urls_for_scraping() {
        assert!(
            SCRAPER_CANDIDATE_QUERY.contains("crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')")
        );
    }

    #[test]
    fn should_select_by_persisted_domain_and_return_one_probe() {
        assert!(SCRAPER_CANDIDATE_QUERY.contains("sd.domain_id = ANY($2)"));
        assert!(SCRAPER_CANDIDATE_QUERY.contains("sd.scrape_failure_streak = 0"));
        assert!(
            SCRAPER_CANDIDATE_QUERY.contains("WHERE (is_domain_probe AND domain_url_rank = 1)")
        );
    }

    /// Recorded evidence (a streak without a cooldown) keeps the whole batch.
    #[test]
    fn should_probe_only_a_domain_whose_cooldown_expired() {
        assert!(SCRAPER_CANDIDATE_QUERY.contains(
            "(sd.scrape_failure_streak > 0\n                AND sd.next_scrape_at IS NOT NULL\n                AND sd.next_scrape_at <= NOW()) AS is_domain_probe"
        ));
    }

    #[test]
    fn should_increment_only_same_kind_domain_failure_streaks() {
        let expected = DomainHealthSnapshot {
            scrape_failure_streak: 2,
            last_scrape_error_kind: Some("HTTP_429".to_owned()),
            next_scrape_at: None,
        };

        assert_eq!(
            next_domain_failure_streak(&expected, DomainFailureKind::Http429),
            3
        );
        assert_eq!(
            next_domain_failure_streak(&expected, DomainFailureKind::Timeout),
            1
        );

        let timeout = DomainHealthSnapshot {
            scrape_failure_streak: 1,
            last_scrape_error_kind: Some("TIMEOUT".to_owned()),
            next_scrape_at: None,
        };
        assert_eq!(
            next_domain_failure_streak(&timeout, DomainFailureKind::Timeout),
            2
        );
    }

    #[test]
    fn should_decode_a_persisted_schema_review_wait() {
        let review_id = CrawlerReviewId::new();

        assert_eq!(schema_review_wait_from_row(None, 0).unwrap(), None);
        assert_eq!(
            schema_review_wait_from_row(Some(*review_id.as_uuid()), 3).unwrap(),
            Some(SchemaReviewWait {
                review_id,
                streak: 3
            })
        );
    }

    #[test]
    fn should_fail_closed_on_an_inconsistent_persisted_schema_review_wait() {
        let review_uuid = *CrawlerReviewId::new().as_uuid();

        for (waited_on, streak) in [
            (None, 2),
            (Some(review_uuid), 0),
            (Some(review_uuid), -1),
            (Some(uuid::Uuid::new_v4()), 1),
        ] {
            assert!(
                matches!(
                    schema_review_wait_from_row(waited_on, streak),
                    Err(sqlx::Error::Decode(_))
                ),
                "{waited_on:?} with streak {streak} must not decode"
            );
        }
    }

    #[test]
    fn should_start_domain_streak_at_one_after_recovery() {
        let expected = DomainHealthSnapshot {
            scrape_failure_streak: 0,
            last_scrape_error_kind: None,
            next_scrape_at: None,
        };

        assert_eq!(
            next_domain_failure_streak(&expected, DomainFailureKind::Http429),
            1
        );
    }
}
