//! The review event trail: one row per accepted review mutation, appended as the last write of
//! the mutation's transaction, so a refused or failed mutation leaves none. The persisted
//! `details` of each action are this module's [`ReviewMutation`] serialized.

use super::{ReviewRepositoryError, crawler_review_id_from_uuid};
use crate::review::model::{CrawlerReviewEvent, ReviewActor, ReviewEventAction};
use crate::review::page_coverage::ReviewPageCoverage;
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use crate::{CrawlerReviewEventId, CrawlerReviewId};
use serde::Serialize;
use sqlx::Row;

/// An accepted review mutation as the event trail records it: the action, the verdict's notes and
/// the details the action keeps. A mutation that replaces the ListingSource's live configuration
/// keeps what it replaced, so that can be restored.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(super) enum ReviewMutation<'a> {
    CandidateEdited {
        /// Whether the edit changed the candidate's removed-page templates, which declare
        /// removed pages.
        removed_page_schemas_changed: bool,
        /// The live schema list an edit through the approved review whose candidate is live
        /// replaced.
        #[serde(skip_serializing_if = "Option::is_none")]
        replaced_product_schemas: Option<Vec<ProductCssSelectorSchema>>,
    },
    SchemaFieldEdited {
        schema_index: usize,
        field: &'a str,
        /// As for [`ReviewMutation::CandidateEdited`].
        #[serde(skip_serializing_if = "Option::is_none")]
        replaced_product_schemas: Option<Vec<ProductCssSelectorSchema>>,
    },
    PageAdded {
        url: &'a str,
        role: &'a str,
    },
    PageRemoved {
        url: String,
        role: String,
    },
    RepairRequested {
        #[serde(skip)]
        note: &'a str,
    },
    ProductSchemasApproved {
        #[serde(skip)]
        notes: Option<&'a str>,
        /// The coverage the approval was accepted under.
        coverage: ReviewPageCoverage,
        /// The live schema list the approval replaced, `null` when none was live.
        replaced_product_schemas: Option<Vec<ProductCssSelectorSchema>>,
        /// The stored removed-page templates the approval replaced.
        replaced_removed_page_schemas: Vec<RemovedPageSchema>,
    },
    UrlPatternApproved {
        #[serde(skip)]
        notes: Option<&'a str>,
        /// The domain's URL-pattern decision the approval replaced.
        replaced_url_pattern: ReplacedUrlPattern,
    },
    Rejected {
        #[serde(skip)]
        notes: Option<&'a str>,
    },
}

/// A domain's URL-pattern decision as `listing_source_domains` stores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ReplacedUrlPattern {
    pub(super) url_pattern: Option<String>,
    pub(super) url_pattern_state: String,
}

impl ReviewMutation<'_> {
    fn action(&self) -> ReviewEventAction {
        match self {
            Self::CandidateEdited { .. } => ReviewEventAction::CandidateEdited,
            Self::SchemaFieldEdited { .. } => ReviewEventAction::SchemaFieldEdited,
            Self::PageAdded { .. } => ReviewEventAction::PageAdded,
            Self::PageRemoved { .. } => ReviewEventAction::PageRemoved,
            Self::RepairRequested { .. } => ReviewEventAction::RepairRequested,
            Self::ProductSchemasApproved { .. } | Self::UrlPatternApproved { .. } => {
                ReviewEventAction::Approved
            }
            Self::Rejected { .. } => ReviewEventAction::Rejected,
        }
    }

    fn notes(&self) -> Option<&str> {
        match self {
            Self::RepairRequested { note } => Some(note),
            Self::ProductSchemasApproved { notes, .. }
            | Self::UrlPatternApproved { notes, .. }
            | Self::Rejected { notes } => *notes,
            Self::CandidateEdited { .. }
            | Self::SchemaFieldEdited { .. }
            | Self::PageAdded { .. }
            | Self::PageRemoved { .. } => None,
        }
    }
}

/// Appends the event of `actor`'s `mutation` of the review and commits the mutation's
/// transaction with it. The caller holds the review's row lock, so the review's events append
/// in the order its mutations commit, each with the candidate version the mutation left.
pub(super) async fn commit_with_event(
    mut transaction: sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    actor: ReviewActor,
    mutation: ReviewMutation<'_>,
) -> Result<(), ReviewRepositoryError> {
    let appended = sqlx::query(
        "INSERT INTO crawler_review_events \
             (event_id, review_id, actor, action, candidate_version, notes, details) \
         SELECT $1, review_id, $3, $4, candidate_version, $5, $6 \
         FROM crawler_reviews WHERE review_id = $2",
    )
    .bind(CrawlerReviewEventId::new().as_uuid())
    .bind(review_id.as_uuid())
    .bind(actor.as_str())
    .bind(mutation.action().as_str())
    .bind(mutation.notes())
    .bind(serde_json::to_value(&mutation)?)
    .execute(&mut *transaction)
    .await?;
    if appended.rows_affected() != 1 {
        return Err(ReviewRepositoryError::NotFound(review_id));
    }
    transaction.commit().await?;
    Ok(())
}

/// The review's events, oldest first. A stored actor or action no event has fails the read.
pub(super) async fn fetch_review_events<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    review_id: CrawlerReviewId,
) -> Result<Vec<CrawlerReviewEvent>, sqlx::Error> {
    sqlx::query(
        "SELECT event_id, review_id, actor, action, candidate_version, notes, details, created \
         FROM crawler_review_events WHERE review_id = $1 \
         ORDER BY created, event_id",
    )
    .bind(review_id.as_uuid())
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(row_to_event)
    .collect()
}

fn row_to_event(row: sqlx::postgres::PgRow) -> Result<CrawlerReviewEvent, sqlx::Error> {
    let event_id: uuid::Uuid = row.try_get("event_id")?;
    let actor: String = row.try_get("actor")?;
    let action: String = row.try_get("action")?;
    Ok(CrawlerReviewEvent {
        event_id: CrawlerReviewEventId::try_from(event_id).map_err(|source| {
            super::persisted_object_id_error("CrawlerReviewEventId", event_id, source)
        })?,
        review_id: crawler_review_id_from_uuid(row.try_get("review_id")?)?,
        actor: ReviewActor::from_persisted(&actor).ok_or_else(|| {
            sqlx::Error::Decode(format!("unknown persisted review event actor `{actor}`").into())
        })?,
        action: ReviewEventAction::from_persisted(&action).ok_or_else(|| {
            sqlx::Error::Decode(format!("unknown persisted review event action `{action}`").into())
        })?,
        candidate_version: row.try_get("candidate_version")?,
        notes: row.try_get("notes")?,
        details: row.try_get("details")?,
        created: row.try_get("created")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn details(mutation: &ReviewMutation<'_>) -> serde_json::Value {
        serde_json::to_value(mutation).unwrap_or_else(|error| panic!("details: {error}"))
    }

    /// The details keep what each action needs to be understood, and what an approval or a live
    /// edit replaced so it can be restored; notes live in their own column, never in the details.
    #[test]
    fn should_keep_the_details_of_each_action_without_its_notes() {
        let coverage = ReviewPageCoverage {
            covered: 2,
            removed: 1,
            total: 3,
            not_evaluated: 1,
        };
        for (mutation, action, notes, expected) in [
            (
                ReviewMutation::CandidateEdited {
                    removed_page_schemas_changed: true,
                    replaced_product_schemas: None,
                },
                ReviewEventAction::CandidateEdited,
                None,
                json!({ "removed_page_schemas_changed": true }),
            ),
            (
                ReviewMutation::SchemaFieldEdited {
                    schema_index: 1,
                    field: "raw_attributes.rawMaterial",
                    replaced_product_schemas: Some(vec![]),
                },
                ReviewEventAction::SchemaFieldEdited,
                None,
                json!({
                    "schema_index": 1,
                    "field": "raw_attributes.rawMaterial",
                    "replaced_product_schemas": [],
                }),
            ),
            (
                ReviewMutation::PageAdded {
                    url: "https://shop.example/p/1",
                    role: "REVIEWER_ADDED",
                },
                ReviewEventAction::PageAdded,
                None,
                json!({ "url": "https://shop.example/p/1", "role": "REVIEWER_ADDED" }),
            ),
            (
                ReviewMutation::RepairRequested {
                    note: "title misses sold pages",
                },
                ReviewEventAction::RepairRequested,
                Some("title misses sold pages"),
                json!({}),
            ),
            (
                ReviewMutation::ProductSchemasApproved {
                    notes: Some("covers the shop"),
                    coverage,
                    replaced_product_schemas: None,
                    replaced_removed_page_schemas: vec![],
                },
                ReviewEventAction::Approved,
                Some("covers the shop"),
                json!({
                    "coverage": { "covered": 2, "removed": 1, "total": 3, "not_evaluated": 1 },
                    "replaced_product_schemas": null,
                    "replaced_removed_page_schemas": [],
                }),
            ),
            (
                ReviewMutation::UrlPatternApproved {
                    notes: None,
                    replaced_url_pattern: ReplacedUrlPattern {
                        url_pattern: None,
                        url_pattern_state: "UNKNOWN".to_owned(),
                    },
                },
                ReviewEventAction::Approved,
                None,
                json!({ "replaced_url_pattern": { "url_pattern": null, "url_pattern_state": "UNKNOWN" } }),
            ),
            (
                ReviewMutation::Rejected {
                    notes: Some("not a shop page"),
                },
                ReviewEventAction::Rejected,
                Some("not a shop page"),
                json!({}),
            ),
        ] {
            assert_eq!(mutation.action(), action);
            assert_eq!(mutation.notes(), notes);
            assert_eq!(details(&mutation), expected, "{action:?}");
        }
    }
}
