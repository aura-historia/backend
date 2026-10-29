//! The sample labels of a URL-pattern review: the author agent or a human says, for URLs of the
//! review's domain, its sample's among them, which are product pages and which are not. A review
//! decides one domain, so labels on another domain of its ListingSource would say nothing about
//! the decision. They belong to the candidate, so setting them bumps its version and is recorded
//! as a candidate edit by its actor. The contract is in `docs/crawler/extraction-and-review.md`
//! (approval guard).

use super::review_events::{ReviewMutation, commit_with_event};
use super::reviewer_page_url::{ReviewPageIdentity, without_credentials};
use super::{
    CrawlerReviewRepository, ReviewRepositoryError, ReviewUnderEdit, lock_review_for_edit,
    store_pending_candidate,
};
use crate::CrawlerReviewId;
use crate::network::policy::url_matches_configured_domain;
use crate::review::model::{
    ARTIFACT_URL_PATTERN, LabelledSampleUrl, ReviewActor, STATUS_PENDING_REVIEW, SampleLabelInput,
    SampleLabels, UrlPatternReviewCandidate,
};
use crate::review::source_url::is_review_source_url;
use crate::spider::utils::url::CrawledUrl;
use std::collections::{HashMap, HashSet};
use time::OffsetDateTime;
use tracing::info;
use url::Url;

/// The most URLs one review's sample labels hold.
pub const MAX_SAMPLE_LABELS: usize = 200;
/// The longest reason a sample label gives, in characters.
pub const MAX_SAMPLE_LABEL_REASON_CHARS: usize = 300;

impl CrawlerReviewRepository {
    /// Replaces the sample labels of the pending URL-pattern review, read at
    /// `expected_candidate_version`, with `labels`, and returns the candidate version after the
    /// change. Only those who change candidates set them: the author agent and humans. An empty
    /// list removes every label.
    pub async fn set_sample_labels(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
        expected_candidate_version: i64,
        labels: Vec<SampleLabelInput>,
    ) -> Result<i64, ReviewRepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let ReviewUnderEdit {
            artifact_type,
            status,
            candidate_payload,
            ..
        } = lock_review_for_edit(
            &mut transaction,
            review_id,
            actor,
            &self.agent_decisions,
            expected_candidate_version,
        )
        .await?;
        if artifact_type != ARTIFACT_URL_PATTERN {
            return Err(ReviewRepositoryError::UnsupportedArtifact(
                review_id,
                artifact_type,
            ));
        }
        if status != STATUS_PENDING_REVIEW {
            return Err(ReviewRepositoryError::NotPending(review_id));
        }
        let sample = sqlx::query_scalar::<_, String>(
            "SELECT url FROM crawler_review_urls WHERE review_id = $1 ORDER BY created, url",
        )
        .bind(review_id.as_uuid())
        .fetch_all(&mut *transaction)
        .await?;
        let domain = url_pattern_review_domain(&mut transaction, review_id).await?;
        let labels = validated_sample_labels(labels, &sample, &domain, OffsetDateTime::now_utc())?;
        let counts = labels.as_ref().map(SampleLabels::counts);
        let candidate_payload = UrlPatternReviewCandidate::payload_with_sample_labels(
            candidate_payload,
            labels.as_ref(),
        )
        .map_err(|_| ReviewRepositoryError::InvalidUrlPatternCandidate)?;
        let candidate_version =
            store_pending_candidate(&mut transaction, review_id, candidate_payload).await?;
        let counts = counts.unwrap_or_default();
        commit_with_event(
            transaction,
            review_id,
            actor,
            ReviewMutation::SampleLabelsEdited {
                sample_labels: counts,
            },
        )
        .await?;
        info!(
            review_id = %review_id,
            actor = %actor,
            product_labels = counts.product,
            not_product_labels = counts.not_product,
            candidate_version,
            "Sample labels of a URL-pattern review set"
        );
        Ok(candidate_version)
    }
}

/// The domain the URL-pattern review `review_id` decides, the only one its labels may name URLs
/// of. A URL-pattern review always names a domain of its ListingSource, so a review without one
/// fails as an invalid stored review.
pub(super) async fn url_pattern_review_domain(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
) -> Result<String, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT d.listing_source_domain FROM crawler_reviews r \
         JOIN listing_source_domains d \
           ON d.listing_source_id = r.listing_source_id AND d.domain_id = r.domain_id \
         WHERE r.review_id = $1",
    )
    .bind(review_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
}

/// The sample labels `inputs` describe, labelled at `labelled_at`, or `None` for no label. Each
/// URL must be on the review's `domain`, on http or https, with or without `www.`, as the crawler
/// treats one domain: one of the review's `sample` URLs, under the crawler's URL spelling, or an
/// http(s) URL on the default port; a URL with credentials is refused either way. A label of a
/// sample URL keeps the sample's spelling, any other URL is stored in the crawler's spelling, so
/// a pattern is matched against it as the spider would. Each page is labelled once, in whichever
/// spelling the crawler treats as that page (http or https, with or without `www.`), so the
/// labels of one page never count as several, and with a reason of at most
/// [`MAX_SAMPLE_LABEL_REASON_CHARS`] characters.
fn validated_sample_labels(
    inputs: Vec<SampleLabelInput>,
    sample: &[String],
    domain: &str,
    labelled_at: OffsetDateTime,
) -> Result<Option<SampleLabels>, ReviewRepositoryError> {
    if inputs.len() > MAX_SAMPLE_LABELS {
        return Err(ReviewRepositoryError::TooManySampleLabels);
    }
    let sample_spellings: HashMap<String, &String> = sample
        .iter()
        .filter_map(|url| Some((crawler_spelling(&Url::parse(url).ok()?), url)))
        .collect();
    let mut labelled = HashSet::new();
    let mut labels = Vec::with_capacity(inputs.len());
    for input in inputs {
        let url = labelled_url(&input.url, &sample_spellings, domain)?;
        let reason = input.reason.trim();
        if reason.is_empty() || reason.chars().count() > MAX_SAMPLE_LABEL_REASON_CHARS {
            return Err(ReviewRepositoryError::InvalidSampleLabelReason(url));
        }
        let page = ReviewPageIdentity::of_stored(&url)
            .ok_or_else(|| ReviewRepositoryError::SampleLabelUrlNotAllowed(url.clone()))?;
        if !labelled.insert(page) {
            return Err(ReviewRepositoryError::DuplicateSampleLabel(url));
        }
        labels.push(LabelledSampleUrl {
            url,
            label: input.label,
            reason: reason.to_owned(),
        });
    }
    Ok((!labels.is_empty()).then_some(SampleLabels {
        labelled_at,
        labels,
    }))
}

/// The URL a label of `raw_url` is stored under, or the refusal of a URL a label may not name.
fn labelled_url(
    raw_url: &str,
    sample_spellings: &HashMap<String, &String>,
    domain: &str,
) -> Result<String, ReviewRepositoryError> {
    let not_allowed = |url: String| Err(ReviewRepositoryError::SampleLabelUrlNotAllowed(url));
    let Ok(url) = Url::parse(raw_url.trim()) else {
        return not_allowed(raw_url.trim().to_owned());
    };
    if !url.username().is_empty() || url.password().is_some() {
        return not_allowed(without_credentials(&url).to_string());
    }
    if !url_matches_configured_domain(&url, domain) {
        return not_allowed(url.to_string());
    }
    let spelling = crawler_spelling(&url);
    if let Some(sample_url) = sample_spellings.get(&spelling) {
        return Ok((*sample_url).clone());
    }
    if is_review_source_url(&url, &[domain.to_owned()]) {
        return Ok(spelling);
    }
    not_allowed(url.to_string())
}

fn crawler_spelling(url: &Url) -> String {
    CrawledUrl::new(url.clone()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::SampleUrlLabel;

    fn input(url: &str, label: SampleUrlLabel) -> SampleLabelInput {
        SampleLabelInput {
            url: url.to_owned(),
            label,
            reason: " seen on the page ".to_owned(),
        }
    }

    fn sample() -> Vec<String> {
        vec![
            "https://antiques.example/item/chair".to_owned(),
            "https://antiques.example/journal/restoring-oak".to_owned(),
        ]
    }

    /// The domain the review decides; its ListingSource also holds `antiques.example.de`.
    const DOMAIN: &str = "antiques.example";

    fn validated(
        inputs: Vec<SampleLabelInput>,
    ) -> Result<Option<SampleLabels>, ReviewRepositoryError> {
        validated_sample_labels(inputs, &sample(), DOMAIN, OffsetDateTime::UNIX_EPOCH)
    }

    fn urls(labels: &SampleLabels) -> Vec<(&str, SampleUrlLabel, &str)> {
        labels
            .labels
            .iter()
            .map(|label| (label.url.as_str(), label.label, label.reason.as_str()))
            .collect()
    }

    /// A label names a sample URL in the sample's spelling, however the client spells it, or
    /// another URL of the review's domain, on http or https, with or without `www.`, in the
    /// crawler's spelling; its reason is kept trimmed.
    #[test]
    fn should_take_sample_urls_and_urls_of_the_review_domain() {
        let labels = validated(vec![
            input(
                "https://antiques.example/item/chair/",
                SampleUrlLabel::Product,
            ),
            input(
                "https://www.antiques.example/item/table#gallery",
                SampleUrlLabel::Product,
            ),
            input(
                "http://antiques.example/impressum",
                SampleUrlLabel::NotProduct,
            ),
        ])
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|| panic!("labels"));

        assert_eq!(
            urls(&labels),
            vec![
                (
                    "https://antiques.example/item/chair",
                    SampleUrlLabel::Product,
                    "seen on the page"
                ),
                (
                    "https://www.antiques.example/item/table",
                    SampleUrlLabel::Product,
                    "seen on the page"
                ),
                (
                    "http://antiques.example/impressum",
                    SampleUrlLabel::NotProduct,
                    "seen on the page"
                ),
            ]
        );
        assert_eq!(labels.labelled_at, OffsetDateTime::UNIX_EPOCH);
    }

    /// No label names a URL outside the review's domain, even on another domain of its
    /// ListingSource, a URL with credentials, on another port or scheme, or one that does not
    /// parse; the refusal never repeats credentials.
    #[test]
    fn should_refuse_a_url_not_on_the_review_domain() {
        for (raw, reported) in [
            (
                "https://elsewhere.example/item/1",
                "https://elsewhere.example/item/1",
            ),
            (
                "https://antiques.example.de/item/1",
                "https://antiques.example.de/item/1",
            ),
            (
                "https://shop.antiques.example/item/1",
                "https://shop.antiques.example/item/1",
            ),
            (
                "https://antiques.example:8443/item/1",
                "https://antiques.example:8443/item/1",
            ),
            (
                "ftp://antiques.example/item/1",
                "ftp://antiques.example/item/1",
            ),
            (
                "https://user:secret@antiques.example/item/chair",
                "https://antiques.example/item/chair",
            ),
            ("/item/1", "/item/1"),
        ] {
            let refused = validated(vec![input(raw, SampleUrlLabel::Product)]);

            assert!(
                matches!(
                    &refused,
                    Err(ReviewRepositoryError::SampleLabelUrlNotAllowed(url)) if url == reported
                ),
                "{raw}: {refused:?}"
            );
        }
    }

    /// The crawler treats a trailing slash, http and https, `www.` and the bare host as one page,
    /// so a second label of the page in another spelling is refused rather than counted again;
    /// another query is another page.
    #[test]
    fn should_refuse_a_url_labelled_twice_under_any_spelling() {
        for (again, reported) in [
            (
                "https://antiques.example/item/chair/",
                "https://antiques.example/item/chair",
            ),
            (
                "https://www.antiques.example/item/chair",
                "https://www.antiques.example/item/chair",
            ),
            (
                "http://antiques.example/item/chair#gallery",
                "http://antiques.example/item/chair",
            ),
        ] {
            let refused = validated(vec![
                input(
                    "https://antiques.example/item/chair",
                    SampleUrlLabel::Product,
                ),
                input(again, SampleUrlLabel::NotProduct),
            ]);

            assert!(
                matches!(
                    &refused,
                    Err(ReviewRepositoryError::DuplicateSampleLabel(url)) if url == reported
                ),
                "{again}: {refused:?}"
            );
        }
        let variants = validated(vec![
            input(
                "https://antiques.example/item/chair",
                SampleUrlLabel::Product,
            ),
            input(
                "https://antiques.example/item/chair?finish=oak",
                SampleUrlLabel::Product,
            ),
        ]);
        assert_eq!(
            variants
                .ok()
                .flatten()
                .map(|labels| labels.counts().product),
            Some(2)
        );
    }

    #[test]
    fn should_refuse_a_label_without_a_short_reason() {
        for reason in [
            String::new(),
            " \n ".to_owned(),
            "x".repeat(MAX_SAMPLE_LABEL_REASON_CHARS + 1),
        ] {
            let refused = validated(vec![SampleLabelInput {
                reason,
                ..input(
                    "https://antiques.example/item/chair",
                    SampleUrlLabel::Product,
                )
            }]);

            assert!(
                matches!(
                    refused,
                    Err(ReviewRepositoryError::InvalidSampleLabelReason(_))
                ),
                "{refused:?}"
            );
        }
    }

    #[test]
    fn should_bound_the_labels_of_a_review_and_take_none_as_no_labels() {
        let too_many = (0..=MAX_SAMPLE_LABELS)
            .map(|index| {
                input(
                    &format!("https://antiques.example/item/{index}"),
                    SampleUrlLabel::Product,
                )
            })
            .collect();

        assert!(matches!(
            validated(too_many),
            Err(ReviewRepositoryError::TooManySampleLabels)
        ));
        assert_eq!(validated(Vec::new()).ok(), Some(None));
    }
}
