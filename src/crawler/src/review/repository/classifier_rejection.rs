//! The rule a rejection resting on the page classifier's verdicts passes: the review agent runner
//! rejects a fresh `manual_schema_required` review whose triggering pages its classifier read as
//! no product pages, and the console admits that rejection only while the review is still the one
//! the classifier judged and none of its pages may be a product the crawler once captured or a
//! page a human revoked a rejection of (`docs/crawler/extraction-and-review.md#page-classifier`).

use super::reviewer_page_url::ReviewPageIdentity;
use super::{ROLES_NO_REJECTION_JUDGES, ReviewRepositoryError};
use crate::review::model::{PageClassification, ReviewActor, ReviewEventAction};
use crate::{CrawlerReviewId, CrawlerReviewPageId};
use listing_source_core::ListingSourceId;
use serde::Serialize;
use sqlx::Row;
use std::collections::{BTreeSet, HashSet};

/// Whether the review aliased `r` is one the page classifier judges: a pending
/// `manual_schema_required` product-schema review that no one changed or judged yet (no event,
/// so no hand-over to a human either) and that opened for triggering pages only, not a source's
/// first schema review, whose seed pages failing as listings point at the URL pattern instead.
/// Binds nothing; its literals are the persisted values the test below names.
macro_rules! classifiable_review {
    () => {
        "(r.artifact_type = 'PRODUCT_SCHEMA' AND r.reason = 'manual_schema_required' \
          AND r.status = 'PENDING_REVIEW' \
          AND NOT EXISTS (SELECT 1 FROM crawler_review_events c WHERE c.review_id = r.review_id) \
          AND EXISTS (SELECT 1 FROM crawler_review_pages c WHERE c.review_id = r.review_id \
                      AND c.role = 'TRIGGERING_GENERATION_PAGE') \
          AND NOT EXISTS (SELECT 1 FROM crawler_review_pages c WHERE c.review_id = r.review_id \
                          AND c.role NOT IN ('TRIGGERING_GENERATION_PAGE', 'REVIEWER_ADDED', \
                                             'WAITING_PAGE')))"
    };
}
pub(crate) use classifiable_review;

/// Whether the event aliased `e` is a rejection that rests on the page classifier's verdicts.
/// Binds nothing.
macro_rules! classifier_rejection_event {
    () => {
        "(e.action = 'REJECTED' AND e.details ? 'classification')"
    };
}
pub(crate) use classifier_rejection_event;

/// Why the console refused a rejection resting on the classifier's verdicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ClassifierRejectionRefusal {
    /// The review is no longer one the classifier judges: someone changed or judged it, or it is
    /// not a `manual_schema_required` review of triggering pages.
    #[error("the review is no longer a fresh manual_schema_required review of triggering pages")]
    NotClassifiable,
    /// The verdicts do not name exactly the review's opening pages.
    #[error("the verdicts do not name exactly the review's seed and triggering pages")]
    PagesNotMatched,
    /// A verdict leaves a product or a removal possible.
    #[error("a verdict is not LISTING or OTHER")]
    VerdictAllowsProduct,
    /// An opening page is, or is the redirect target of, a URL that captured a product.
    #[error("a page is a URL that captured a product, or the redirect target of one")]
    PageCapturedProduct,
    /// A human revoked a rejection of an opening page before.
    #[error("a human revoked a rejection of a page before")]
    RejectionRevokedBefore,
}

/// Refuses a rejection of `actor` that carries `classification` unless the actor is the reviewer
/// agent, whose token the runner sends it with: a human's or the author's rejection never rests
/// on the classifier, so neither leaves the loop guard's count.
pub(super) fn admit_classification_actor(
    actor: ReviewActor,
    classification: Option<&PageClassification>,
) -> Result<(), ReviewRepositoryError> {
    if classification.is_some() && actor != ReviewActor::AgentReviewer {
        return Err(ReviewRepositoryError::ClassificationNeedsReviewerAgent);
    }
    Ok(())
}

/// Admits the rejection of the review, whose row lock the caller holds, on `classification`, or
/// refuses it with the first rule it fails: the review must still be classifiable, the verdicts
/// must name exactly its opening pages and rule a product out on each, and no opening page may
/// match, in any spelling, a URL of the ListingSource that captured a product or its redirect
/// target, or an opening page of another review whose rejection a human revoked.
pub(super) async fn admit_classifier_rejection(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    listing_source_id: &ListingSourceId,
    classification: &PageClassification,
) -> Result<(), ReviewRepositoryError> {
    let refuse =
        |refusal| Err(ReviewRepositoryError::ClassifierRejectionRefused { review_id, refusal });
    if !is_classifiable(transaction, review_id).await? {
        return refuse(ClassifierRejectionRefusal::NotClassifiable);
    }
    let opening = opening_pages(transaction, review_id).await?;
    if opening.keys() != classification.page_ids() {
        return refuse(ClassifierRejectionRefusal::PagesNotMatched);
    }
    if !classification.rules_out_every_product() {
        return refuse(ClassifierRejectionRefusal::VerdictAllowsProduct);
    }
    let urls = opening.urls();
    if any_captured_product(transaction, listing_source_id, &urls).await? {
        return refuse(ClassifierRejectionRefusal::PageCapturedProduct);
    }
    if any_revoked_before(transaction, review_id, listing_source_id, &urls).await? {
        return refuse(ClassifierRejectionRefusal::RejectionRevokedBefore);
    }
    Ok(())
}

async fn is_classifiable(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(concat!(
        "SELECT ",
        classifiable_review!(),
        " FROM crawler_reviews r WHERE r.review_id = $1"
    ))
    .bind(review_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
}

/// The review's opening pages, those a rejection classifies: every page but reviewer-added and
/// waiting ones.
struct OpeningPages(Vec<(CrawlerReviewPageId, String)>);

impl OpeningPages {
    fn keys(&self) -> BTreeSet<CrawlerReviewPageId> {
        self.0.iter().map(|(id, _)| *id).collect()
    }

    fn urls(&self) -> Vec<String> {
        self.0.iter().map(|(_, url)| url.clone()).collect()
    }
}

async fn opening_pages(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
) -> Result<OpeningPages, ReviewRepositoryError> {
    let rows = sqlx::query(
        "SELECT review_page_id, url FROM crawler_review_pages \
         WHERE review_id = $1 AND role <> ALL($2)",
    )
    .bind(review_id.as_uuid())
    .bind(ROLES_NO_REJECTION_JUDGES)
    .fetch_all(&mut **transaction)
    .await?;
    let mut pages = Vec::with_capacity(rows.len());
    for row in rows {
        let id: uuid::Uuid = row.try_get("review_page_id")?;
        let id = CrawlerReviewPageId::try_from(id).map_err(|source| {
            super::persisted_object_id_error("CrawlerReviewPageId", id, source)
        })?;
        pages.push((id, row.try_get("url")?));
    }
    Ok(OpeningPages(pages))
}

/// Whether a URL of the ListingSource that captured a product names one of `page_urls`, in any
/// spelling of its page, as its own URL or as the redirect target it waits on or was removed by:
/// such a page may show a removal or a new product template, never a category to retire.
async fn any_captured_product(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    listing_source_id: &ListingSourceId,
    page_urls: &[String],
) -> Result<bool, sqlx::Error> {
    let spellings: Vec<String> = page_urls
        .iter()
        .flat_map(|url| ReviewPageIdentity::stored_spellings(url))
        .collect();
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM listing_source_urls u \
         WHERE u.listing_source_id = $1 \
           AND (u.last_source_listing_id IS NOT NULL \
                OR u.last_captured_raw_input_sha256 IS NOT NULL) \
           AND (u.url = ANY($2::text[]) OR u.unsettled_redirect_target = ANY($2::text[]) \
                OR u.removed_by_redirect_target = ANY($2::text[])))",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(&spellings)
    .fetch_one(&mut **transaction)
    .await
}

/// Whether one of `page_urls` is, in any spelling, an opening page of another review of the
/// ListingSource whose rejection a human revoked: the classifier never undoes that revocation.
async fn any_revoked_before(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    listing_source_id: &ListingSourceId,
    page_urls: &[String],
) -> Result<bool, sqlx::Error> {
    let revoked = sqlx::query_scalar::<_, String>(
        "SELECT p.url FROM crawler_reviews r \
         JOIN crawler_review_pages p ON p.review_id = r.review_id \
         WHERE r.listing_source_id = $1 AND r.review_id <> $2 \
           AND r.artifact_type = 'PRODUCT_SCHEMA' AND p.role <> ALL($3) \
           AND EXISTS (SELECT 1 FROM crawler_review_events e \
                       WHERE e.review_id = r.review_id AND e.action = $4)",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(review_id.as_uuid())
    .bind(ROLES_NO_REJECTION_JUDGES)
    .bind(ReviewEventAction::RejectionRevoked.as_str())
    .fetch_all(&mut **transaction)
    .await?;
    let revoked: HashSet<ReviewPageIdentity> = revoked
        .iter()
        .filter_map(|url| ReviewPageIdentity::of_stored(url))
        .collect();
    Ok(page_urls
        .iter()
        .filter_map(|url| ReviewPageIdentity::of_stored(url))
        .any(|page| revoked.contains(&page)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::{
        ARTIFACT_PRODUCT_SCHEMA, PAGE_ROLE_REVIEWER_ADDED, PAGE_ROLE_TRIGGERING_GENERATION_PAGE,
        PAGE_ROLE_WAITING_PAGE, STATUS_PENDING_REVIEW,
    };
    use crate::scraper::scraper_service::extraction::manual_schema_review::MANUAL_SCHEMA_REASON;

    #[test]
    fn should_name_the_persisted_values_the_classifiable_condition_matches() {
        let condition = classifiable_review!();
        for persisted in [
            ARTIFACT_PRODUCT_SCHEMA,
            MANUAL_SCHEMA_REASON,
            STATUS_PENDING_REVIEW,
            PAGE_ROLE_TRIGGERING_GENERATION_PAGE,
            PAGE_ROLE_REVIEWER_ADDED,
            PAGE_ROLE_WAITING_PAGE,
        ] {
            assert!(condition.contains(&format!("'{persisted}'")), "{persisted}");
        }
        assert_eq!(
            ROLES_NO_REJECTION_JUDGES,
            [PAGE_ROLE_REVIEWER_ADDED, PAGE_ROLE_WAITING_PAGE]
        );
        assert!(
            classifier_rejection_event!()
                .contains(&format!("'{}'", ReviewEventAction::Rejected.as_str()))
        );
    }

    #[rstest::rstest]
    #[case::human(ReviewActor::Human)]
    #[case::author(ReviewActor::AgentAuthor)]
    #[case::system(ReviewActor::System)]
    fn should_refuse_a_classification_from_any_actor_but_the_reviewer_agent(
        #[case] actor: ReviewActor,
    ) {
        let classification: PageClassification = serde_json::from_value(serde_json::json!({
            "pages": [{
                "review_page_id": CrawlerReviewPageId::new().to_string(),
                "verdict": "LISTING",
                "reason": "Lists items.",
            }]
        }))
        .expect("classification");

        assert!(matches!(
            admit_classification_actor(actor, Some(&classification)),
            Err(ReviewRepositoryError::ClassificationNeedsReviewerAgent)
        ));
        assert!(admit_classification_actor(actor, None).is_ok());
        assert!(
            admit_classification_actor(ReviewActor::AgentReviewer, Some(&classification)).is_ok()
        );
    }

    #[test]
    fn should_name_refusals_in_screaming_snake_case() {
        assert_eq!(
            serde_json::to_value(ClassifierRejectionRefusal::PageCapturedProduct).ok(),
            Some(serde_json::json!("PAGE_CAPTURED_PRODUCT"))
        );
    }
}
