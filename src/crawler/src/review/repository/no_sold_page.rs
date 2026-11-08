//! The author's record that it looked for a sold or unavailable page of the ListingSource and
//! found none, which lets the [approval guard](crate::review::approval_guard) accept a candidate
//! tested on no such page until a human next acts on the review. The record is an event of the
//! review's trail that names the URLs the author checked, each one an agent
//! [inspected](super::inspected_pages) for the review since a human last acted on it; it changes
//! no candidate, so it never counts as authoring for the four-eyes rule. The contract is in
//! `docs/crawler/extraction-and-review.md` (approval guard).

use super::inspected_pages::pages_inspected_since_last_human_event;
use super::review_events::{ReviewMutation, commit_with_event};
use super::reviewer_page_url::{ReviewPageIdentity, without_credentials};
use super::{
    CrawlerReviewRepository, ReviewRepositoryError, ReviewUnderEdit, lock_review_for_change,
};
use crate::CrawlerReviewId;
use crate::review::model::{ARTIFACT_PRODUCT_SCHEMA, ReviewActor, STATUS_PENDING_REVIEW};
use crate::review::source_url::is_review_source_url;
use crate::spider::utils::url::CrawledUrl;
use std::collections::HashSet;
use tracing::info;
use url::Url;

/// The fewest distinct pages a record names: one page shows too little of a shop to say it has no
/// sold page.
pub const MIN_CHECKED_URLS: usize = 3;
/// The most pages a record names.
pub const MAX_CHECKED_URLS: usize = 50;
/// The longest account a record keeps, in characters.
pub const MAX_NO_SOLD_PAGE_NOTE_CHARS: usize = 500;

impl CrawlerReviewRepository {
    /// Records that `actor` checked `urls` of the pending product-schema review's ListingSource
    /// and found no sold or unavailable page, with its account in `notes`. Only those who change
    /// candidates record it: the author agent and humans. The author agent names only pages the
    /// console projected for the review since a human last acted on it; a human's record is their
    /// own account. The review's candidate stays as it is.
    pub async fn record_no_sold_page(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
        urls: Vec<String>,
        notes: &str,
    ) -> Result<usize, ReviewRepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let (
            ReviewUnderEdit {
                artifact_type,
                status,
                ..
            },
            _,
        ) = lock_review_for_change(&mut transaction, review_id, actor, &self.agent_decisions)
            .await?;
        if artifact_type != ARTIFACT_PRODUCT_SCHEMA {
            return Err(ReviewRepositoryError::UnsupportedArtifact(
                review_id,
                artifact_type,
            ));
        }
        if status != STATUS_PENDING_REVIEW {
            return Err(ReviewRepositoryError::NotPending(review_id));
        }
        let crawl_host = sqlx::query_scalar::<_, Option<String>>(
            "SELECT s.crawl_host FROM crawler_reviews r \
             JOIN listing_sources s ON s.listing_source_id = r.listing_source_id \
             WHERE r.review_id = $1",
        )
        .bind(review_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await?
        .flatten();
        let checked_urls = validated_checked_urls(urls, crawl_host.as_deref())?;
        if actor.is_agent() {
            let inspected =
                pages_inspected_since_last_human_event(&mut *transaction, review_id).await?;
            refuse_uninspected_pages(&checked_urls, &inspected)?;
        }
        let notes = notes.trim();
        if notes.is_empty() || notes.chars().count() > MAX_NO_SOLD_PAGE_NOTE_CHARS {
            return Err(ReviewRepositoryError::InvalidNoSoldPageNote);
        }
        let recorded = checked_urls.len();
        commit_with_event(
            transaction,
            review_id,
            actor,
            ReviewMutation::NoSoldPageRecorded {
                notes,
                checked_urls,
            },
        )
        .await?;
        info!(
            review_id = %review_id,
            actor = %actor,
            checked_urls = recorded,
            "No sold or unavailable page recorded for a product-schema review"
        );
        Ok(recorded)
    }
}

/// The `urls` a record names, each in the crawler's spelling: between [`MIN_CHECKED_URLS`] and
/// [`MAX_CHECKED_URLS`] distinct pages, each an http(s) URL on the default port, without
/// credentials, on the `crawl_host` of the review's ListingSource. A page named twice, under any
/// spelling the crawler treats as one page, counts once.
fn validated_checked_urls(
    urls: Vec<String>,
    crawl_host: Option<&str>,
) -> Result<Vec<String>, ReviewRepositoryError> {
    if urls.len() > MAX_CHECKED_URLS {
        return Err(ReviewRepositoryError::NoSoldPageUrlCount);
    }
    let mut pages = HashSet::new();
    let mut checked = Vec::with_capacity(urls.len());
    for raw in urls {
        let not_allowed =
            |url: &str| ReviewRepositoryError::ReviewPageUrlNotAllowed(url.to_owned());
        let url = Url::parse(raw.trim()).map_err(|_| not_allowed(raw.trim()))?;
        if !is_review_source_url(&url, crawl_host) {
            return Err(not_allowed(without_credentials(&url).as_str()));
        }
        if pages.insert(ReviewPageIdentity::of(&url)) {
            checked.push(CrawledUrl::new(url).to_string());
        }
    }
    if checked.len() < MIN_CHECKED_URLS {
        return Err(ReviewRepositoryError::NoSoldPageUrlCount);
    }
    Ok(checked)
}

/// Refuses a record that names a page outside `inspected`, naming every such page.
fn refuse_uninspected_pages(
    checked_urls: &[String],
    inspected: &HashSet<ReviewPageIdentity>,
) -> Result<(), ReviewRepositoryError> {
    let uninspected: Vec<String> = checked_urls
        .iter()
        .filter(|url| {
            !ReviewPageIdentity::of_stored(url).is_some_and(|page| inspected.contains(&page))
        })
        .cloned()
        .collect();
    if uninspected.is_empty() {
        Ok(())
    } else {
        Err(ReviewRepositoryError::NoSoldPageUrlsNotInspected(
            uninspected,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRAWL_HOST: Option<&str> = Some("antiques.example");

    fn urls(count: usize) -> Vec<String> {
        (0..count)
            .map(|index| format!("https://antiques.example/item/{index}"))
            .collect()
    }

    #[test]
    fn should_take_distinct_pages_on_the_crawl_host_in_the_crawlers_spelling() {
        let checked = validated_checked_urls(
            vec![
                " https://antiques.example/item/chair ".to_owned(),
                "https://www.antiques.example/item/chair/".to_owned(),
                "http://antiques.example/item/table#gallery".to_owned(),
                "https://antiques.example/item/lamp".to_owned(),
                "https://antiques.example/item/vase".to_owned(),
            ],
            CRAWL_HOST,
        )
        .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(checked.len(), 4, "{checked:?}");
        assert_eq!(checked[0], "https://antiques.example/item/chair");
    }

    #[test]
    fn should_refuse_too_few_or_too_many_pages() {
        for count in [0, MIN_CHECKED_URLS - 1, MAX_CHECKED_URLS + 1] {
            assert!(
                matches!(
                    validated_checked_urls(urls(count), CRAWL_HOST),
                    Err(ReviewRepositoryError::NoSoldPageUrlCount)
                ),
                "{count}"
            );
        }
        let one_page_thrice = vec!["https://antiques.example/item/chair".to_owned(); 3];
        assert!(matches!(
            validated_checked_urls(one_page_thrice, CRAWL_HOST),
            Err(ReviewRepositoryError::NoSoldPageUrlCount)
        ));
        assert!(validated_checked_urls(urls(MIN_CHECKED_URLS), CRAWL_HOST).is_ok());
    }

    #[test]
    fn should_refuse_a_url_off_the_crawl_host_or_with_credentials_without_repeating_them() {
        for (raw, reported) in [
            (
                "https://elsewhere.example/item/1",
                "https://elsewhere.example/item/1",
            ),
            (
                "https://user:secret@antiques.example/item/1",
                "https://antiques.example/item/1",
            ),
            ("/item/1", "/item/1"),
        ] {
            let mut checked = urls(3);
            checked.push(raw.to_owned());

            assert!(
                matches!(
                    validated_checked_urls(checked, CRAWL_HOST),
                    Err(ReviewRepositoryError::ReviewPageUrlNotAllowed(url)) if url == reported
                ),
                "{raw}"
            );
        }
        assert!(validated_checked_urls(urls(3), None).is_err());
    }

    #[test]
    fn should_refuse_a_record_naming_a_page_nobody_inspected_in_any_spelling() {
        let inspected: HashSet<ReviewPageIdentity> = [
            "http://www.antiques.example/item/0/",
            "https://antiques.example/item/1",
        ]
        .into_iter()
        .filter_map(ReviewPageIdentity::of_stored)
        .collect();

        assert!(refuse_uninspected_pages(&urls(2), &inspected).is_ok());
        assert!(matches!(
            refuse_uninspected_pages(&urls(4), &inspected),
            Err(ReviewRepositoryError::NoSoldPageUrlsNotInspected(uninspected))
                if uninspected == ["https://antiques.example/item/2", "https://antiques.example/item/3"]
        ));
    }
}
