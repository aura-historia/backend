//! The pages a ListingSource's rejected manual product-schema reviews classify as non-product
//! pages (`docs/crawler/operations.md#manual-authoring`): the one place that decides whether a
//! rejection covers a crawler URL, for the scraper's check of one page and for every write that
//! classifies stored URLs by their URL pattern.

use super::reviewer_page_url::ReviewPageIdentity;
use super::{ROLES_NO_REJECTION_JUDGES, ReviewEventAction};
use crate::CrawlerReviewId;
use crate::scraper::scraper_service::extraction::manual_schema_review::MANUAL_SCHEMA_REASON;
use listing_source_core::ListingSourceId;
use std::collections::HashSet;

/// The pages unrevoked rejections of a ListingSource's reviews cover, each as the console's
/// [`ReviewPageIdentity`], so a crawler URL in any spelling of a rejected page is covered.
#[derive(Debug, Default)]
pub(crate) struct RejectedPages {
    pages: HashSet<ReviewPageIdentity>,
}

impl RejectedPages {
    /// The pages that rejections of `listing_source_id`'s `manual_schema_required` reviews cover.
    pub(crate) async fn load<'e>(
        executor: impl sqlx::PgExecutor<'e>,
        listing_source_id: &ListingSourceId,
    ) -> Result<Self, sqlx::Error> {
        Self::load_for_reason(executor, listing_source_id, MANUAL_SCHEMA_REASON, None).await
    }

    /// The pages the other rejections cover while the rejection of `revoked` is being revoked, in
    /// the transaction that records the revocation.
    pub(super) async fn load_without<'e>(
        executor: impl sqlx::PgExecutor<'e>,
        listing_source_id: &ListingSourceId,
        revoked: CrawlerReviewId,
    ) -> Result<Self, sqlx::Error> {
        Self::load_for_reason(
            executor,
            listing_source_id,
            MANUAL_SCHEMA_REASON,
            Some(revoked),
        )
        .await
    }

    /// The pages that rejections of `listing_source_id`'s product-schema reviews opened for
    /// `reason` cover: their seed and triggering pages, never pages that judge no rejection (such
    /// as reviewer-added pages), while no human revoked the rejection; the review `except`, when
    /// given, does not count.
    pub(super) async fn load_for_reason<'e>(
        executor: impl sqlx::PgExecutor<'e>,
        listing_source_id: &ListingSourceId,
        reason: &str,
        except: Option<CrawlerReviewId>,
    ) -> Result<Self, sqlx::Error> {
        let urls = sqlx::query_scalar::<_, String>(
            "SELECT p.url FROM crawler_reviews r \
             JOIN crawler_review_pages p ON p.review_id = r.review_id \
             WHERE r.listing_source_id = $1 AND r.artifact_type = 'PRODUCT_SCHEMA' \
               AND r.status = 'REJECTED' AND r.reason = $2 AND p.role <> ALL($3) \
               AND r.review_id IS DISTINCT FROM $5 \
               AND NOT EXISTS (SELECT 1 FROM crawler_review_events e \
                               WHERE e.review_id = r.review_id AND e.action = $4)",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(reason)
        .bind(ROLES_NO_REJECTION_JUDGES)
        .bind(ReviewEventAction::RejectionRevoked.as_str())
        .bind(except.as_ref().map(CrawlerReviewId::as_uuid))
        .fetch_all(executor)
        .await?;
        Ok(Self::of_page_urls(&urls))
    }

    fn of_page_urls(urls: &[String]) -> Self {
        Self {
            pages: urls
                .iter()
                .filter_map(|url| ReviewPageIdentity::of_stored(url))
                .collect(),
        }
    }

    /// Whether a rejection covers the page of the stored crawler URL `url`. A value that is not a
    /// URL names no page and is never covered.
    pub(crate) fn covers(&self, url: &str) -> bool {
        !self.pages.is_empty()
            && ReviewPageIdentity::of_stored(url).is_some_and(|page| self.pages.contains(&page))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rejection covers its page in every spelling the console treats as that page, and no
    /// other page.
    #[test]
    fn should_cover_every_spelling_of_a_rejected_page_and_no_other_page() {
        let rejected = RejectedPages::of_page_urls(&[
            "https://www.example.com/products/1".to_owned(),
            "https://example.com/products/2/?q=1".to_owned(),
            "not a url".to_owned(),
        ]);

        for covered in [
            "https://www.example.com/products/1",
            "https://www.example.com/products/1/",
            "http://example.com/products/1",
            "https://example.com/products/1#gallery",
            "https://example.com/products/2?q=1",
            "http://www.example.com/products/2/?q=1",
        ] {
            assert!(rejected.covers(covered), "{covered}");
        }
        for other in [
            "https://www.example.com/products/10",
            "https://www.example.com/products/2",
            "https://www.example.com/products/1?variant=sold",
            "https://other.example/products/1",
            "not a url",
        ] {
            assert!(!rejected.covers(other), "{other}");
        }
    }
}
