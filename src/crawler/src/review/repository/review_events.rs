//! The review event trail: one row per accepted review mutation, appended as the last write of
//! the mutation's transaction, so a refused or failed mutation leaves none. The persisted
//! `details` of each action are this module's [`ReviewMutation`] serialized.

use super::{ReviewRepositoryError, crawler_review_id_from_uuid};
use crate::review::model::{
    CrawlerReviewEvent, LiveMatrixEvaluation, ReviewActor, ReviewEventAction, ReviewEventSummary,
    SampleLabelCounts, SampleLabels,
};
use crate::review::page_coverage::ReviewPageCoverage;
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use crate::{CrawlerReviewEventId, CrawlerReviewId};
use regex::Regex;
use serde::{Deserialize, Serialize};
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
    /// `POST /api/reviews/{id}/sample-labels`, recorded as a candidate edit: the labels belong to
    /// the URL-pattern candidate.
    SampleLabelsEdited {
        /// How many URLs the labels set mark as products and as no products.
        sample_labels: SampleLabelCounts,
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
        /// What the stored live matrix the approval was accepted under evaluated, when it has
        /// one.
        #[serde(skip_serializing_if = "Option::is_none")]
        live_evaluation: Option<LiveMatrixEvaluation>,
        #[serde(flatten)]
        configuration: ProductSchemaApprovalRecord,
    },
    UrlPatternApproved {
        #[serde(skip)]
        notes: Option<&'a str>,
        /// The candidate's sample labels the approval was accepted on, when it has any.
        #[serde(skip_serializing_if = "Option::is_none")]
        sample_labels: Option<SampleLabels>,
        #[serde(flatten)]
        configuration: UrlPatternApprovalRecord,
    },
    Rejected {
        #[serde(skip)]
        notes: Option<&'a str>,
    },
    ProductSchemasRestored {
        #[serde(skip)]
        notes: Option<&'a str>,
        /// The approval whose replaced configuration went live again.
        restored_event_id: CrawlerReviewEventId,
        /// The live schema list the restore replaced: the one the approval made live.
        replaced_product_schemas: Vec<ProductCssSelectorSchema>,
        /// The stored removed-page templates the restore replaced.
        replaced_removed_page_schemas: Vec<RemovedPageSchema>,
    },
    UrlPatternRestored {
        #[serde(skip)]
        notes: Option<&'a str>,
        /// As for [`ReviewMutation::ProductSchemasRestored`].
        restored_event_id: CrawlerReviewEventId,
        /// The domain's URL-pattern decision the restore replaced.
        replaced_url_pattern: DomainUrlPattern,
    },
    RejectionRevoked {
        #[serde(skip)]
        notes: Option<&'a str>,
        /// The crawler URLs of the rejected pages whose class the revocation changed.
        reclassified_urls: Vec<String>,
    },
}

/// A domain's URL-pattern decision as `listing_source_domains` stores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct DomainUrlPattern {
    pub(super) url_pattern: Option<String>,
    pub(super) url_pattern_state: String,
}

impl DomainUrlPattern {
    /// The pattern that classifies the domain's URLs as products: only a `MATCHED` decision has
    /// one.
    pub(super) fn live_pattern(&self) -> Result<Option<Regex>, regex::Error> {
        self.url_pattern
            .as_deref()
            .filter(|_| self.url_pattern_state == "MATCHED")
            .map(Regex::new)
            .transpose()
    }
}

/// The live product-schema configuration an approval replaced and the one it made live, as the
/// approval's event keeps them, so a restore can bring back the first while the second is still
/// live.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct ProductSchemaApprovalRecord {
    /// The live schema list the approval replaced, `null` when none was live.
    pub(super) replaced_product_schemas: Option<Vec<ProductCssSelectorSchema>>,
    /// The stored removed-page templates the approval replaced.
    pub(super) replaced_removed_page_schemas: Vec<RemovedPageSchema>,
    /// The schema list the approval made live.
    pub(super) approved_product_schemas: Vec<ProductCssSelectorSchema>,
    /// The removed-page templates the approval stored.
    pub(super) approved_removed_page_schemas: Vec<RemovedPageSchema>,
}

/// As [`ProductSchemaApprovalRecord`], for a domain's URL-pattern decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct UrlPatternApprovalRecord {
    pub(super) replaced_url_pattern: DomainUrlPattern,
    pub(super) approved_url_pattern: DomainUrlPattern,
}

impl ReviewMutation<'_> {
    fn action(&self) -> ReviewEventAction {
        match self {
            Self::CandidateEdited { .. } | Self::SampleLabelsEdited { .. } => {
                ReviewEventAction::CandidateEdited
            }
            Self::SchemaFieldEdited { .. } => ReviewEventAction::SchemaFieldEdited,
            Self::PageAdded { .. } => ReviewEventAction::PageAdded,
            Self::PageRemoved { .. } => ReviewEventAction::PageRemoved,
            Self::RepairRequested { .. } => ReviewEventAction::RepairRequested,
            Self::ProductSchemasApproved { .. } | Self::UrlPatternApproved { .. } => {
                ReviewEventAction::Approved
            }
            Self::Rejected { .. } => ReviewEventAction::Rejected,
            Self::ProductSchemasRestored { .. } | Self::UrlPatternRestored { .. } => {
                ReviewEventAction::Restored
            }
            Self::RejectionRevoked { .. } => ReviewEventAction::RejectionRevoked,
        }
    }

    fn notes(&self) -> Option<&str> {
        match self {
            Self::RepairRequested { note } => Some(note),
            Self::ProductSchemasApproved { notes, .. }
            | Self::UrlPatternApproved { notes, .. }
            | Self::Rejected { notes }
            | Self::ProductSchemasRestored { notes, .. }
            | Self::UrlPatternRestored { notes, .. }
            | Self::RejectionRevoked { notes, .. } => *notes,
            Self::CandidateEdited { .. }
            | Self::SampleLabelsEdited { .. }
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

/// The approval event of a review, as a restore reads it.
pub(super) struct RecordedApproval {
    pub(super) event_id: CrawlerReviewEventId,
    /// The approval's details, which keep what it replaced and what it made live.
    pub(super) details: serde_json::Value,
    /// Whether a restore already brought back what the approval replaced.
    pub(super) restored: bool,
}

/// The review's approval event, `None` when the review was never approved. A review is approved
/// at most once, and restored at most once after that.
pub(super) async fn fetch_recorded_approval<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    review_id: CrawlerReviewId,
) -> Result<Option<RecordedApproval>, sqlx::Error> {
    let events = sqlx::query_as::<_, (uuid::Uuid, String, serde_json::Value)>(
        "SELECT event_id, action, details FROM crawler_review_events \
         WHERE review_id = $1 AND action = ANY($2) \
         ORDER BY created, event_id",
    )
    .bind(review_id.as_uuid())
    .bind([
        ReviewEventAction::Approved.as_str(),
        ReviewEventAction::Restored.as_str(),
    ])
    .fetch_all(executor)
    .await?;
    let restored = events
        .iter()
        .any(|(_, action, _)| action == ReviewEventAction::Restored.as_str());
    events
        .into_iter()
        .rev()
        .find(|(_, action, _)| action == ReviewEventAction::Approved.as_str())
        .map(|(event_id, _, details)| {
            Ok(RecordedApproval {
                event_id: CrawlerReviewEventId::try_from(event_id).map_err(|source| {
                    super::persisted_object_id_error("CrawlerReviewEventId", event_id, source)
                })?,
                details,
                restored,
            })
        })
        .transpose()
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

/// The events of all reviews created at or after `since`, oldest first, at most `limit`, each
/// with its review's artifact type and ListingSource and without its details, and whether later
/// events were left out.
pub(super) async fn fetch_review_events_since<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    since: time::OffsetDateTime,
    limit: usize,
) -> Result<(Vec<ReviewEventSummary>, bool), sqlx::Error> {
    let rows = sqlx::query(
        "SELECT e.event_id, e.review_id, r.artifact_type, r.listing_source_id, \
                s.listing_source_name, e.actor, e.action, e.candidate_version, e.notes, e.created \
         FROM crawler_review_events e \
         JOIN crawler_reviews r ON r.review_id = e.review_id \
         LEFT JOIN listing_sources s ON s.listing_source_id = r.listing_source_id \
         WHERE e.created >= $1 \
         ORDER BY e.created, e.event_id \
         LIMIT $2",
    )
    .bind(since)
    .bind(i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX))
    .fetch_all(executor)
    .await?;
    let truncated = rows.len() > limit;
    let events = rows
        .into_iter()
        .take(limit)
        .map(|row| {
            let event_id: uuid::Uuid = row.try_get("event_id")?;
            let actor: String = row.try_get("actor")?;
            let action: String = row.try_get("action")?;
            Ok(ReviewEventSummary {
                event_id: CrawlerReviewEventId::try_from(event_id).map_err(|source| {
                    super::persisted_object_id_error("CrawlerReviewEventId", event_id, source)
                })?,
                review_id: crawler_review_id_from_uuid(row.try_get("review_id")?)?,
                artifact_type: row.try_get("artifact_type")?,
                listing_source_id: super::listing_source_id_from_uuid(
                    row.try_get("listing_source_id")?,
                )?,
                listing_source_name: row.try_get("listing_source_name")?,
                actor: persisted_actor(&actor)?,
                action: persisted_action(&action)?,
                candidate_version: row.try_get("candidate_version")?,
                notes: row.try_get("notes")?,
                created: row.try_get("created")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    Ok((events, truncated))
}

fn persisted_actor(actor: &str) -> Result<ReviewActor, sqlx::Error> {
    ReviewActor::from_persisted(actor).ok_or_else(|| {
        sqlx::Error::Decode(format!("unknown persisted review event actor `{actor}`").into())
    })
}

fn persisted_action(action: &str) -> Result<ReviewEventAction, sqlx::Error> {
    ReviewEventAction::from_persisted(action).ok_or_else(|| {
        sqlx::Error::Decode(format!("unknown persisted review event action `{action}`").into())
    })
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
        actor: persisted_actor(&actor)?,
        action: persisted_action(&action)?,
        candidate_version: row.try_get("candidate_version")?,
        notes: row.try_get("notes")?,
        details: row.try_get("details")?,
        created: row.try_get("created")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::{LabelledSampleUrl, SampleUrlLabel};
    use serde_json::json;

    fn details(mutation: &ReviewMutation<'_>) -> serde_json::Value {
        serde_json::to_value(mutation).unwrap_or_else(|error| panic!("details: {error}"))
    }

    fn url_pattern_record() -> UrlPatternApprovalRecord {
        UrlPatternApprovalRecord {
            replaced_url_pattern: DomainUrlPattern {
                url_pattern: None,
                url_pattern_state: "UNKNOWN".to_owned(),
            },
            approved_url_pattern: DomainUrlPattern {
                url_pattern: Some("/item/".to_owned()),
                url_pattern_state: "MATCHED".to_owned(),
            },
        }
    }

    fn sample_labels() -> SampleLabels {
        SampleLabels {
            labelled_at: time::OffsetDateTime::UNIX_EPOCH,
            labels: vec![LabelledSampleUrl {
                url: "https://shop.example/item/1".to_owned(),
                label: SampleUrlLabel::Product,
                reason: "one oak chair with price".to_owned(),
            }],
        }
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
        let event_id = CrawlerReviewEventId::new();
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
                ReviewMutation::SampleLabelsEdited {
                    sample_labels: SampleLabelCounts {
                        product: 3,
                        not_product: 4,
                    },
                },
                ReviewEventAction::CandidateEdited,
                None,
                json!({ "sample_labels": { "product": 3, "not_product": 4 } }),
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
                    live_evaluation: None,
                    configuration: ProductSchemaApprovalRecord {
                        replaced_product_schemas: None,
                        replaced_removed_page_schemas: vec![],
                        approved_product_schemas: vec![],
                        approved_removed_page_schemas: vec![],
                    },
                },
                ReviewEventAction::Approved,
                Some("covers the shop"),
                json!({
                    "coverage": { "covered": 2, "removed": 1, "total": 3, "not_evaluated": 1 },
                    "replaced_product_schemas": null,
                    "replaced_removed_page_schemas": [],
                    "approved_product_schemas": [],
                    "approved_removed_page_schemas": [],
                }),
            ),
            (
                ReviewMutation::UrlPatternApproved {
                    notes: None,
                    sample_labels: None,
                    configuration: url_pattern_record(),
                },
                ReviewEventAction::Approved,
                None,
                json!({
                    "replaced_url_pattern": { "url_pattern": null, "url_pattern_state": "UNKNOWN" },
                    "approved_url_pattern": { "url_pattern": "/item/", "url_pattern_state": "MATCHED" },
                }),
            ),
            (
                ReviewMutation::UrlPatternApproved {
                    notes: None,
                    sample_labels: Some(sample_labels()),
                    configuration: url_pattern_record(),
                },
                ReviewEventAction::Approved,
                None,
                json!({
                    "sample_labels": {
                        "labelled_at": "1970-01-01T00:00:00Z",
                        "labels": [{
                            "url": "https://shop.example/item/1",
                            "label": "PRODUCT",
                            "reason": "one oak chair with price",
                        }],
                    },
                    "replaced_url_pattern": { "url_pattern": null, "url_pattern_state": "UNKNOWN" },
                    "approved_url_pattern": { "url_pattern": "/item/", "url_pattern_state": "MATCHED" },
                }),
            ),
            (
                ReviewMutation::UrlPatternRestored {
                    notes: Some("agent pattern too broad"),
                    restored_event_id: event_id,
                    replaced_url_pattern: DomainUrlPattern {
                        url_pattern: Some("/item/".to_owned()),
                        url_pattern_state: "MATCHED".to_owned(),
                    },
                },
                ReviewEventAction::Restored,
                Some("agent pattern too broad"),
                json!({
                    "restored_event_id": event_id,
                    "replaced_url_pattern": { "url_pattern": "/item/", "url_pattern_state": "MATCHED" },
                }),
            ),
            (
                ReviewMutation::Rejected {
                    notes: Some("not a shop page"),
                },
                ReviewEventAction::Rejected,
                Some("not a shop page"),
                json!({}),
            ),
            (
                ReviewMutation::RejectionRevoked {
                    notes: Some("the agent rejected product pages"),
                    reclassified_urls: vec!["https://shop.example/item/1".to_owned()],
                },
                ReviewEventAction::RejectionRevoked,
                Some("the agent rejected product pages"),
                json!({ "reclassified_urls": ["https://shop.example/item/1"] }),
            ),
        ] {
            assert_eq!(mutation.action(), action);
            assert_eq!(mutation.notes(), notes);
            assert_eq!(details(&mutation), expected, "{action:?}");
        }
    }

    /// A restore reads back what an approval's event keeps, whatever else the event records.
    #[test]
    fn should_read_back_the_configuration_an_approval_recorded() {
        let configuration = ProductSchemaApprovalRecord {
            replaced_product_schemas: None,
            replaced_removed_page_schemas: vec![],
            approved_product_schemas: vec![],
            approved_removed_page_schemas: vec![],
        };
        let approval = ReviewMutation::ProductSchemasApproved {
            notes: None,
            coverage: ReviewPageCoverage {
                covered: 1,
                removed: 0,
                total: 1,
                not_evaluated: 0,
            },
            live_evaluation: None,
            configuration: configuration.clone(),
        };

        let url_pattern_approval = ReviewMutation::UrlPatternApproved {
            notes: None,
            sample_labels: Some(sample_labels()),
            configuration: url_pattern_record(),
        };

        let read = serde_json::from_value::<ProductSchemaApprovalRecord>(details(&approval));
        let read_url_pattern =
            serde_json::from_value::<UrlPatternApprovalRecord>(details(&url_pattern_approval));

        assert_eq!(read.ok(), Some(configuration));
        assert_eq!(read_url_pattern.ok(), Some(url_pattern_record()));
        assert!(
            serde_json::from_value::<ProductSchemaApprovalRecord>(json!({
                "coverage": { "covered": 1, "removed": 0, "total": 1, "not_evaluated": 0 },
                "replaced_product_schemas": null,
                "replaced_removed_page_schemas": [],
            }))
            .is_err(),
            "an approval that kept only what it replaced cannot be restored"
        );
    }
}
