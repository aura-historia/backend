//! Staging a structured-data draft in place of a manual review's starter schema, the placeholder
//! the review opens with. Only an untouched starter is replaced, so a draft never overwrites what
//! an agent or a human wrote (`docs/crawler/extraction-and-review.md`, structured-data draft).

use super::schema_payload::ProductSchemaReviewCandidate;
use super::{CrawlerReviewRepository, ReviewRepositoryError};
use crate::CrawlerReviewId;
use crate::review::model::{ARTIFACT_PRODUCT_SCHEMA, CrawlerReview, ReviewActor};
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::scraper_service::extraction::manual_schema_review::starter_schema;
use serde_json::json;

impl CrawlerReviewRepository {
    /// Replaces the untouched starter schema of the review with `draft` as `actor`'s candidate
    /// edit, and answers the new candidate version; `None` when the review holds no untouched
    /// starter, which the draft then leaves alone. The edit names the version it read, so a
    /// concurrent change refuses it with `CONFLICT`.
    pub async fn stage_structured_draft(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
        draft: ProductCssSelectorSchema,
    ) -> Result<Option<i64>, ReviewRepositoryError> {
        let review = self.get_review(review_id).await?.review;
        let Some(starter_index) = untouched_starter_index(&review)? else {
            return Ok(None);
        };
        let mut schemas = ProductSchemaReviewCandidate::read(&review.candidate_payload)?.schemas;
        schemas[starter_index] = draft;
        self.update_candidate_payload(
            review_id,
            actor,
            review.candidate_version,
            json!({ "schemas": schemas }),
        )
        .await
        .map(Some)
    }
}

/// The index of the review's starter schema while it is still the placeholder the review opened
/// with, or `None` for a review that holds none: no manual product-schema review, or a starter
/// someone already changed.
fn untouched_starter_index(review: &CrawlerReview) -> Result<Option<usize>, ReviewRepositoryError> {
    if review.artifact_type != ARTIFACT_PRODUCT_SCHEMA
        || review.validation_summary["manual_authoring"] != true
    {
        return Ok(None);
    }
    let Some(index) = review.validation_summary["starter_schema_index"]
        .as_u64()
        .and_then(|index| usize::try_from(index).ok())
    else {
        return Ok(None);
    };
    let schemas = ProductSchemaReviewCandidate::read(&review.candidate_payload)?.schemas;
    Ok((schemas.get(index) == Some(&starter_schema())).then_some(index))
}
