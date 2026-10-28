//! The ListingSource a review belongs to, as far as the console fetches pages for the review: its
//! domains, and the URLs on them a page inspection, a candidate evaluation or a review agent's live
//! fetch may name.

use super::reviewer_page_url::{reviewer_page_url, without_credentials};
use super::{CrawlerReviewRepository, MAX_REVIEWER_ADDED_PAGES, ReviewRepositoryError};
use crate::CrawlerReviewId;
use crate::review::source_url::is_review_source_url;
use sqlx::PgPool;
use url::Url;

/// The most ListingSource URLs one candidate evaluation fetches next to the review's pages. Like
/// the reviewer-added pages, each costs a fetch at the shop's crawl delay.
pub const MAX_EVALUATED_URLS: usize = MAX_REVIEWER_ADDED_PAGES;

impl CrawlerReviewRepository {
    /// `raw_url` as a URL of the review's ListingSource the console may fetch for the review, its
    /// fragment dropped, or the refusal of a review that does not exist or a URL that is not a
    /// plain http(s) URL on one of the ListingSource's domains.
    pub async fn review_source_url(
        &self,
        review_id: CrawlerReviewId,
        raw_url: &str,
    ) -> Result<Url, ReviewRepositoryError> {
        let listing_source_id = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT listing_source_id FROM crawler_reviews WHERE review_id = $1",
        )
        .bind(review_id.as_uuid())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(ReviewRepositoryError::NotFound(review_id))?;
        let domains = listing_source_domains(&self.pool, listing_source_id).await?;
        review_source_url(raw_url, &domains)
    }
}

/// The domains of the ListingSource `listing_source_id`.
pub(super) async fn listing_source_domains(
    pool: &PgPool,
    listing_source_id: uuid::Uuid,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT listing_source_domain FROM listing_source_domains WHERE listing_source_id = $1",
    )
    .bind(listing_source_id)
    .fetch_all(pool)
    .await
}

/// `raw_url` without its fragment when it is a plain http(s) URL on one of `domains`; otherwise
/// the refusal names it without credentials.
pub(super) fn review_source_url(
    raw_url: &str,
    domains: &[String],
) -> Result<Url, ReviewRepositoryError> {
    let url = Url::parse(raw_url.trim())
        .map_err(|_| ReviewRepositoryError::ReviewPageUrlNotAllowed(raw_url.trim().to_owned()))?;
    if !is_review_source_url(&url, domains) {
        return Err(ReviewRepositoryError::ReviewPageUrlNotAllowed(
            without_credentials(&url).to_string(),
        ));
    }
    Ok(reviewer_page_url(&url))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domains() -> Vec<String> {
        vec!["antiques.example".to_owned()]
    }

    #[test]
    fn should_take_a_url_of_the_source_without_its_fragment() {
        let url = review_source_url(" https://www.antiques.example/item/1#gallery ", &domains())
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(url.as_str(), "https://www.antiques.example/item/1");
    }

    #[test]
    fn should_refuse_a_url_off_the_source_naming_it_without_credentials() {
        for (raw, named) in [
            (
                "https://other.example/item/1",
                "https://other.example/item/1",
            ),
            (
                "https://user:secret@antiques.example/item/1",
                "https://antiques.example/item/1",
            ),
            (
                "http://antiques.example:8080/item/1",
                "http://antiques.example:8080/item/1",
            ),
            ("not a url", "not a url"),
        ] {
            match review_source_url(raw, &domains()) {
                Err(ReviewRepositoryError::ReviewPageUrlNotAllowed(url)) => {
                    assert_eq!(url, named, "{raw}");
                }
                other => panic!("{raw}: {other:?}"),
            }
        }
    }
}
