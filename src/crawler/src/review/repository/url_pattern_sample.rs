//! The sample of a URL-pattern review: its URLs as the review's URL rows store them, classified
//! under the ListingSource's current pattern and under the candidate's, and how many of them the
//! candidate takes as product pages. The contract is in `docs/crawler/extraction-and-review.md`
//! (review rail).

use super::reviewer_page_url::ReviewPageIdentity;
use crate::review::approval_guard::SampleUrl;
use crate::review::model::UrlPatternReviewCandidate;
use crate::spider::classification::product_url_pattern::ProductUrlPattern;
use crate::spider::classification::url_classifier::UrlClassifier;
use crate::spider::utils::url::CrawledUrl;
use crate::{CrawlerReviewId, CrawlerReviewUrlId};
use url::Url;

/// The most URLs a URL-pattern review's sample holds: the crawl's first URLs when the review
/// opened, and then the URLs the crawl stored while the review waits, see [`refresh_sample`].
pub(super) const MAX_SAMPLE_URLS: i64 = 1_000;
/// How long a refresh of a review's sample waits for the next one, in seconds.
const SAMPLE_REFRESH_INTERVAL_SECS: f64 = 60.0;

/// How one sample URL classifies under the candidate's pattern.
pub(super) struct CandidateClassification {
    /// Whether the candidate's pattern takes the URL; `None` when the candidate has no pattern.
    pub(super) pattern_match: Option<bool>,
    /// The URL's class as the spider of the authoring mode classifies it under that pattern.
    pub(super) class: String,
}

impl CandidateClassification {
    pub(super) fn of(
        url_classifier: UrlClassifier,
        raw_url: &str,
        candidate_pattern: Option<&ProductUrlPattern>,
    ) -> Self {
        Self {
            pattern_match: candidate_pattern.map(|pattern| pattern_takes(pattern, raw_url)),
            class: url_classifier
                .classify(raw_url, candidate_pattern)
                .to_string(),
        }
    }
}

/// Records what inspecting the page of `url` found, whether it describes exactly one schema.org
/// `Product`, on the sample row of the review that holds the page under any spelling the crawler
/// stores it under. A URL outside the sample, and any review without a sample, record nothing.
/// The approval guard compares the share of such pages with what the pattern takes.
pub(super) async fn record_sample_page_markup(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    url: &Url,
    describes_one_product: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE crawler_review_urls SET describes_one_product = $3 \
         WHERE review_id = $1 AND url = ANY($2)",
    )
    .bind(review_id.as_uuid())
    .bind(ReviewPageIdentity::spellings(url))
    .bind(describes_one_product)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// The URLs of the review's sample with what their inspection found, as the approval guard reads
/// them.
pub(super) async fn fetch_sample_urls<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    review_id: CrawlerReviewId,
) -> Result<Vec<SampleUrl>, sqlx::Error> {
    Ok(sqlx::query_as::<_, (String, Option<bool>)>(
        "SELECT url, describes_one_product FROM crawler_review_urls \
         WHERE review_id = $1 ORDER BY created, url",
    )
    .bind(review_id.as_uuid())
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(|(url, describes_one_product)| SampleUrl {
        url,
        describes_one_product,
    })
    .collect())
}

/// Classifies the stored sample of the review `review_id` again under the edited candidate's
/// `candidate_pattern` and records how many of its URLs that pattern takes as the validation
/// summary's `candidate_product_count`, in the transaction that stores the candidate, so the URL
/// rows always describe the candidate the review holds. The review row is locked by the edit.
pub(super) async fn reclassify_sample(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    url_classifier: UrlClassifier,
    candidate_pattern: Option<&ProductUrlPattern>,
) -> Result<(), sqlx::Error> {
    let sample: Vec<(uuid::Uuid, String)> =
        sqlx::query_as("SELECT review_url_id, url FROM crawler_review_urls WHERE review_id = $1")
            .bind(review_id.as_uuid())
            .fetch_all(&mut **transaction)
            .await?;
    let mut review_url_ids = Vec::with_capacity(sample.len());
    let mut pattern_matches = Vec::with_capacity(sample.len());
    let mut classes = Vec::with_capacity(sample.len());
    for (review_url_id, url) in &sample {
        let candidate = CandidateClassification::of(url_classifier, url, candidate_pattern);
        review_url_ids.push(*review_url_id);
        pattern_matches.push(candidate.pattern_match);
        classes.push(candidate.class);
    }
    sqlx::query(
        "UPDATE crawler_review_urls AS review_url \
         SET candidate_pattern_match = candidate.pattern_match, candidate_class = candidate.class \
         FROM UNNEST($2::uuid[], $3::boolean[], $4::text[]) \
              AS candidate(review_url_id, pattern_match, class) \
         WHERE review_url.review_id = $1 AND review_url.review_url_id = candidate.review_url_id",
    )
    .bind(review_id.as_uuid())
    .bind(&review_url_ids)
    .bind(&pattern_matches)
    .bind(&classes)
    .execute(&mut **transaction)
    .await?;
    let urls: Vec<String> = sample.into_iter().map(|(_, url)| url).collect();
    let product_count = i64::try_from(candidate_product_count(&urls, candidate_pattern))
        .map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
    sqlx::query(
        "UPDATE crawler_reviews \
         SET validation_summary = jsonb_set(validation_summary, '{candidate_product_count}', to_jsonb($2::bigint)) \
         WHERE review_id = $1",
    )
    .bind(review_id.as_uuid())
    .bind(product_count)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Adds the URLs the crawl stored for the pending URL-pattern review's ListingSource since the
/// review opened to its sample, up to [`MAX_SAMPLE_URLS`] in all, and updates the sample counts of
/// the validation summary. A URL not yet in the sample is picked in the order of its hash, not of
/// its insertion, so the crawl's later URLs of every path shape are in reach and not only the last
/// ones. The URLs take their `previous_class` from the stored class and their match flags from
/// the review's current and candidate patterns, as the rows of the opening sample do. It runs at
/// most once per [`SAMPLE_REFRESH_INTERVAL_SECS`] for a review and is a no-op for a review that is
/// not a pending URL-pattern review or whose candidate does not decode, which fail closed
/// elsewhere. The review row is locked, as every edit of its sample or labels does.
pub(super) async fn refresh_sample(
    pool: &sqlx::PgPool,
    review_id: CrawlerReviewId,
    url_classifier: UrlClassifier,
) -> Result<(), sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let due: Option<(uuid::Uuid, serde_json::Value)> = sqlx::query_as(
        "SELECT listing_source_id, candidate_payload FROM crawler_reviews \
         WHERE review_id = $1 AND artifact_type = 'URL_PATTERN' AND status = 'PENDING_REVIEW' \
           AND COALESCE((validation_summary->>'sample_refreshed_at')::timestamptz, '-infinity') \
               <= NOW() - make_interval(secs => $2) \
         FOR UPDATE",
    )
    .bind(review_id.as_uuid())
    .bind(SAMPLE_REFRESH_INTERVAL_SECS)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some((listing_source_id, candidate_payload)) = due else {
        return Ok(());
    };
    let Ok(candidate) = serde_json::from_value::<UrlPatternReviewCandidate>(candidate_payload)
    else {
        return Ok(());
    };
    let (Ok(candidate_pattern), Ok(current_pattern)) =
        (candidate.decision.candidate_pattern(), candidate.current())
    else {
        return Ok(());
    };
    let present: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM crawler_review_urls WHERE review_id = $1")
            .bind(review_id.as_uuid())
            .fetch_one(&mut *transaction)
            .await?;
    let room = (MAX_SAMPLE_URLS - present).max(0);
    let new_urls: Vec<(String, String)> = sqlx::query_as(
        "SELECT url, url_class FROM listing_source_urls source_url \
         WHERE listing_source_id = $1 \
           AND NOT EXISTS (SELECT 1 FROM crawler_review_urls sampled \
                           WHERE sampled.review_id = $2 AND sampled.url = source_url.url) \
         ORDER BY md5(url) LIMIT $3",
    )
    .bind(listing_source_id)
    .bind(review_id.as_uuid())
    .bind(room)
    .fetch_all(&mut *transaction)
    .await?;
    if !new_urls.is_empty() {
        insert_refreshed_urls(
            &mut transaction,
            review_id,
            &new_urls,
            url_classifier,
            current_pattern.as_ref(),
            candidate_pattern.as_ref(),
        )
        .await?;
    }
    sqlx::query(
        "UPDATE crawler_reviews SET validation_summary = validation_summary || jsonb_build_object( \
             'sample_count', (SELECT COUNT(*) FROM crawler_review_urls WHERE review_id = $1), \
             'candidate_product_count', (SELECT COUNT(*) FROM crawler_review_urls \
                                         WHERE review_id = $1 AND candidate_pattern_match), \
             'sample_refreshed_at', to_jsonb(NOW())) \
         WHERE review_id = $1",
    )
    .bind(review_id.as_uuid())
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await
}

async fn insert_refreshed_urls(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    urls: &[(String, String)],
    url_classifier: UrlClassifier,
    current_pattern: Option<&ProductUrlPattern>,
    candidate_pattern: Option<&ProductUrlPattern>,
) -> Result<(), sqlx::Error> {
    let mut review_url_ids = Vec::with_capacity(urls.len());
    let mut raw_urls = Vec::with_capacity(urls.len());
    let mut previous_classes = Vec::with_capacity(urls.len());
    let mut current_matches = Vec::with_capacity(urls.len());
    let mut candidate_matches = Vec::with_capacity(urls.len());
    let mut candidate_classes = Vec::with_capacity(urls.len());
    for (url, stored_class) in urls {
        let candidate = CandidateClassification::of(url_classifier, url, candidate_pattern);
        review_url_ids.push(*CrawlerReviewUrlId::new().as_uuid());
        raw_urls.push(url.as_str());
        previous_classes.push(stored_class.as_str());
        current_matches.push(current_pattern.map(|pattern| pattern_takes(pattern, url)));
        candidate_matches.push(candidate.pattern_match);
        candidate_classes.push(candidate.class);
    }
    sqlx::query(
        "INSERT INTO crawler_review_urls ( \
             review_url_id, review_id, url, previous_class, current_pattern_match, \
             candidate_pattern_match, candidate_class) \
         SELECT added.review_url_id, $1, added.url, added.previous_class, added.current_match, \
                added.candidate_match, added.candidate_class \
         FROM UNNEST($2::uuid[], $3::text[], $4::text[], $5::boolean[], $6::boolean[], $7::text[]) \
              AS added(review_url_id, url, previous_class, current_match, candidate_match, candidate_class)",
    )
    .bind(review_id.as_uuid())
    .bind(&review_url_ids)
    .bind(&raw_urls)
    .bind(&previous_classes)
    .bind(&current_matches)
    .bind(&candidate_matches)
    .bind(&candidate_classes)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// How many of `urls` the candidate's pattern takes; none without a pattern.
pub(super) fn candidate_product_count(
    urls: &[String],
    candidate_pattern: Option<&ProductUrlPattern>,
) -> usize {
    candidate_pattern
        .map(|pattern| {
            urls.iter()
                .filter(|url| pattern_takes(pattern, url))
                .count()
        })
        .unwrap_or(0)
}

/// Whether `pattern` takes the stored URL `raw_url` as the spider matches it, with or without its
/// trailing slash.
pub(super) fn pattern_takes(pattern: &ProductUrlPattern, raw_url: &str) -> bool {
    CrawledUrl::try_from(raw_url).is_ok_and(|url| url.matches_pattern(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::UrlPatternDecision;
    use crate::review::repository::CrawlerReviewRepository;
    use listing_source_core::ListingSourceId;
    use regex::Regex;
    use sqlx::PgPool;
    use test_api::IntegrationTestService as _;

    const SHOP: &str = "https://sample-shop.example";

    /// A ListingSource whose crawl stored `stored` URLs, `/item/<n>` and `/shop/<n>` alternating,
    /// and its pending review of a candidate taking `/item/`, opened with the first two.
    async fn review_after_crawl(
        pool: &PgPool,
        stored: i32,
    ) -> (CrawlerReviewRepository, CrawlerReviewId, ListingSourceId) {
        let listing_source_id = ListingSourceId::new();
        sqlx::query(
            "INSERT INTO listing_sources \
             (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated) \
             VALUES ($1, 'Sample shop', 'sample-shop', TRUE, NOW(), NOW())",
        )
        .bind(listing_source_id.as_uuid())
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert listing source: {error}"));
        store_urls(pool, listing_source_id, 0, stored).await;
        let repository = CrawlerReviewRepository::new(pool.clone());
        let pattern = Regex::new("/item/").unwrap_or_else(|error| panic!("pattern: {error}"));
        let opening_sample = [format!("{SHOP}/item/0"), format!("{SHOP}/shop/1")];
        let review_id = repository
            .create_url_pattern_review(
                &listing_source_id,
                "manual_url_pattern_required",
                UrlPatternDecision::inferred(Some(&pattern)),
                &opening_sample,
                None,
            )
            .await
            .unwrap_or_else(|error| panic!("create review: {error}"));
        (repository, review_id, listing_source_id)
    }

    async fn store_urls(pool: &PgPool, listing_source_id: ListingSourceId, from: i32, to: i32) {
        sqlx::query(
            "INSERT INTO listing_source_urls (listing_source_id, url, url_class, raw_source_record_key) \
             SELECT $1, $2 || CASE WHEN n % 2 = 0 THEN '/item/' ELSE '/shop/' END || n, 'product', \
                    $2 || '/' || n \
             FROM generate_series($3::int, $4::int - 1) AS n \
             ON CONFLICT DO NOTHING",
        )
        .bind(listing_source_id.as_uuid())
        .bind(SHOP)
        .bind(from)
        .bind(to)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("store urls: {error}"));
    }

    async fn allow_next_refresh(pool: &PgPool, review_id: CrawlerReviewId) {
        sqlx::query(
            "UPDATE crawler_reviews SET validation_summary = validation_summary - 'sample_refreshed_at' \
             WHERE review_id = $1",
        )
        .bind(review_id.as_uuid())
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("allow refresh: {error}"));
    }

    #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
    async fn should_add_the_urls_stored_after_the_review_opened_once_a_minute() {
        let pool = test_api::get_postgres_client().await;
        let (repository, review_id, listing_source_id) = review_after_crawl(&pool, 10).await;

        repository
            .refresh_url_pattern_sample(review_id)
            .await
            .unwrap_or_else(|error| panic!("refresh: {error}"));
        store_urls(&pool, listing_source_id, 10, 14).await;
        repository
            .refresh_url_pattern_sample(review_id)
            .await
            .unwrap_or_else(|error| panic!("second refresh: {error}"));

        let detail = repository
            .get_review(review_id)
            .await
            .unwrap_or_else(|error| panic!("get review: {error}"));
        let mut urls: Vec<&str> = detail.urls.iter().map(|url| url.url.as_str()).collect();
        assert_eq!(
            &urls[..2],
            [&format!("{SHOP}/item/0"), &format!("{SHOP}/shop/1")]
        );
        urls.sort_unstable();
        urls.dedup();
        assert_eq!(urls.len(), 10, "the 10 stored URLs, each once: {urls:?}");
        let added = detail
            .urls
            .iter()
            .find(|url| url.url == format!("{SHOP}/item/2"))
            .unwrap_or_else(|| panic!("an added URL"));
        assert_eq!(
            (
                added.previous_class.as_deref(),
                added.current_pattern_match,
                added.candidate_pattern_match,
                added.candidate_class.as_deref()
            ),
            (Some("product"), None, Some(true), Some("product"))
        );
        assert_eq!(detail.review.validation_summary["sample_count"], 10);
        assert_eq!(
            detail.review.validation_summary["candidate_product_count"],
            5
        );

        allow_next_refresh(&pool, review_id).await;
        repository
            .refresh_url_pattern_sample(review_id)
            .await
            .unwrap_or_else(|error| panic!("third refresh: {error}"));
        let count = repository
            .get_review(review_id)
            .await
            .unwrap_or_else(|error| panic!("get review: {error}"))
            .urls
            .len();
        assert_eq!(count, 14);
    }

    #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
    async fn should_stop_adding_urls_at_the_sample_cap() {
        let pool = test_api::get_postgres_client().await;
        let (repository, review_id, _) =
            review_after_crawl(&pool, MAX_SAMPLE_URLS as i32 + 200).await;

        repository
            .refresh_url_pattern_sample(review_id)
            .await
            .unwrap_or_else(|error| panic!("refresh: {error}"));
        allow_next_refresh(&pool, review_id).await;
        repository
            .refresh_url_pattern_sample(review_id)
            .await
            .unwrap_or_else(|error| panic!("second refresh: {error}"));

        let count = repository
            .get_review(review_id)
            .await
            .unwrap_or_else(|error| panic!("get review: {error}"))
            .urls
            .len();
        assert_eq!(count as i64, MAX_SAMPLE_URLS);
    }
}
