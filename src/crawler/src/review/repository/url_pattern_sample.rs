//! The sample of a URL-pattern review: its URLs as the review's URL rows store them, classified
//! under the domain's current pattern and under the candidate's, and how many of them the
//! candidate takes as product pages. The contract is in `docs/crawler/extraction-and-review.md`
//! (review rail).

use crate::CrawlerReviewId;
use crate::spider::classification::url_classifier::UrlClassifier;
use crate::spider::utils::url::CrawledUrl;
use regex::Regex;

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
        candidate_pattern: Option<&Regex>,
    ) -> Self {
        Self {
            pattern_match: candidate_pattern.map(|pattern| pattern_takes(pattern, raw_url)),
            class: url_classifier
                .classify(raw_url, candidate_pattern)
                .to_string(),
        }
    }
}

/// Classifies the stored sample of the review `review_id` again under the edited candidate's
/// `candidate_pattern` and records how many of its URLs that pattern takes as the validation
/// summary's `candidate_product_count`, in the transaction that stores the candidate, so the URL
/// rows always describe the candidate the review holds. The review row is locked by the edit.
pub(super) async fn reclassify_sample(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    url_classifier: UrlClassifier,
    candidate_pattern: Option<&Regex>,
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

/// How many of `urls` the candidate's pattern takes; none without a pattern.
pub(super) fn candidate_product_count(urls: &[String], candidate_pattern: Option<&Regex>) -> usize {
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
pub(super) fn pattern_takes(pattern: &Regex, raw_url: &str) -> bool {
    CrawledUrl::try_from(raw_url).is_ok_and(|url| url.matches_pattern(pattern))
}
