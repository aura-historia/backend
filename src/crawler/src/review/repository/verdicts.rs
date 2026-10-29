//! The verdicts a pending review takes: approval, which makes its candidate the live
//! configuration, rejection and the needs-repair verdict. Each locks the review first, so
//! verdicts, edits and page joins on it apply one after the other. The review agents' approvals
//! and rejections pass the actor's role, [`AgentDecisionPolicy`] and, for approvals, the four-eyes
//! rule and the [approval guard](crate::review::approval_guard) here, and no agent verdict is
//! taken on a review that waits for a human, so no caller decides for an agent without them.

use super::repair_rounds::refuse_agent_while_human_required;
use super::review_events::{
    DomainUrlPattern, ProductSchemaApprovalRecord, ReviewMutation, UrlPatternApprovalRecord,
    commit_with_event,
};
use super::sample_labels::url_pattern_review_domain;
use super::schema_payload::ProductSchemaReviewCandidate;
use super::{
    CrawlerReviewRepository, ReviewRepositoryError, crawler_domain_id_from_uuid,
    fetch_review_pages, listing_source_id_from_uuid, lock_live_product_schemas,
    persist_product_schemas,
};
use crate::review::agent_decisions::{AgentApprovals, AgentDecisionPolicy};
use crate::review::approval_guard::{
    CurrentReview, FailedApprovalCheck, product_schema_approval_checks, url_pattern_approval_checks,
};
use crate::review::model::*;
use crate::review::page_coverage::{IncompleteCoverage, ReviewPageCoverage};
use crate::review::schema_evaluation::stored_schema_matrix;
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use crate::scraper::css_selector::removed_page_schema_repository::{
    lock_stored_removed_page_schemas, store_removed_page_schemas,
};
use crate::spider::candidate_service::AWAITING_URL_PATTERN_REVIEW_CRAWL_OUTCOME;
use crate::spider::classification::url_classifier::UrlClassifier;
use crate::{CrawlerDomainId, CrawlerReviewId};
use listing_source_core::ListingSourceId;
use regex::Regex;
use serde::Deserialize;
use sqlx::Row;
use std::num::NonZeroU32;
use std::time::Duration;
use time::OffsetDateTime;
use tracing::info;

/// An approval of a pending review as its approver sends it.
#[derive(Debug, Clone, Copy)]
pub struct Approval<'a> {
    /// The candidate version the approver read. An approval of any other version is refused, so
    /// an edit made since is never approved unseen.
    pub expected_candidate_version: i64,
    pub notes: Option<&'a str>,
    /// Whether a product-schema candidate that does not cover every review page is approved;
    /// URL patterns have no page coverage and ignore it.
    pub incomplete_coverage: IncompleteCoverage,
}

impl CrawlerReviewRepository {
    /// Makes the pending review's candidate, at the approved candidate version, the active
    /// configuration. A product-schema candidate that does not cover every review page is
    /// approved only with [`IncompleteCoverage::Acknowledged`] of the review's current coverage,
    /// which a review agent never sends; an agent's approval also passes the four-eyes rule and
    /// the approval guard. Approval moves the review past the approved candidate version, so an
    /// edit based on the pending review no longer applies. The approval's event keeps the live
    /// configuration it replaced and the one it made live.
    pub async fn approve_review(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
        approval: Approval<'_>,
    ) -> Result<(), ReviewRepositoryError> {
        admit_approval(&self.agent_decisions, actor, approval.incomplete_coverage)?;
        let mut transaction = self.pool.begin().await?;
        let review =
            lock_review_for_verdict(&mut transaction, review_id, actor, &self.agent_decisions)
                .await?;
        review.check_candidate_version(review_id, approval.expected_candidate_version)?;
        let guarded = actor.is_agent();
        if guarded {
            check_agent_decision_rate(
                &mut transaction,
                self.agent_decisions.max_decisions_per_hour,
            )
            .await?;
            refuse_approval_of_own_change(&mut transaction, review_id, actor, &review).await?;
        }

        let event = match review.artifact_type.as_str() {
            ARTIFACT_URL_PATTERN => {
                let candidate = UrlPatternReviewCandidate::deserialize(&review.candidate_payload)
                    .map_err(|_| ReviewRepositoryError::InvalidUrlPatternCandidate)?;
                if guarded {
                    let domain = url_pattern_review_domain(&mut transaction, review_id).await?;
                    refuse_failed_checks(
                        review_id,
                        url_pattern_approval_checks(
                            &candidate,
                            &domain,
                            self.agent_decisions.min_labelled_urls,
                        ),
                    )?;
                }
                let configuration = make_reviewed_url_pattern_live(
                    &mut transaction,
                    review_id,
                    &review,
                    &candidate,
                    self.url_classifier,
                )
                .await?;
                ReviewMutation::UrlPatternApproved {
                    notes: approval.notes,
                    sample_labels: candidate.sample_labels,
                    configuration,
                }
            }
            ARTIFACT_PRODUCT_SCHEMA => {
                let going_live = lock_product_schemas_going_live(&mut transaction, &review).await?;
                let pages = fetch_review_pages(&mut *transaction, review_id).await?;
                let matrix = stored_schema_matrix(&review.validation_summary).and_then(Result::ok);
                if guarded {
                    guard_agent_product_schema_approval(
                        review_id,
                        &review,
                        &pages,
                        matrix.as_ref(),
                        &going_live.live,
                        self.agent_decisions.max_matrix_age,
                    )?;
                }
                let coverage = check_approval_coverage(
                    review_id,
                    matrix.as_ref(),
                    &pages,
                    approval.incomplete_coverage,
                )?;
                ReviewMutation::ProductSchemasApproved {
                    notes: approval.notes,
                    coverage,
                    live_evaluation: matrix.and_then(|matrix| matrix.live_evaluation),
                    configuration: make_product_schemas_live(
                        &mut transaction,
                        review_id,
                        &review.listing_source_id,
                        going_live,
                    )
                    .await?,
                }
            }
            other => {
                return Err(ReviewRepositoryError::UnsupportedArtifact(
                    review_id,
                    other.into(),
                ));
            }
        };

        close_approved_review(&mut transaction, review_id, &review, approval).await?;
        release_work_waiting_on(&mut transaction, &review.waiting_work()).await?;
        commit_with_event(transaction, review_id, actor, event).await?;
        info!(review_id = %review_id, actor = %actor, "Review approved");
        Ok(())
    }

    /// Closes the review without changing configuration and, as approval does, releases the work
    /// waiting on it: the review is decided, so a page that waited on a product-schema review is
    /// retried at once and the next crawl of a domain that waited on a URL-pattern review may open
    /// a new one. A review agent's rejection says why in its notes.
    pub async fn reject_review(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
        notes: Option<&str>,
    ) -> Result<(), ReviewRepositoryError> {
        admit_rejection(&self.agent_decisions, actor, notes)?;
        let mut transaction = self.pool.begin().await?;
        let review =
            lock_review_for_verdict(&mut transaction, review_id, actor, &self.agent_decisions)
                .await?;
        if actor.is_agent() {
            check_agent_decision_rate(
                &mut transaction,
                self.agent_decisions.max_decisions_per_hour,
            )
            .await?;
        }
        sqlx::query(
            "UPDATE crawler_reviews \
             SET status = 'REJECTED', reviewer_notes = COALESCE($2, reviewer_notes), \
                 reviewed = NOW(), updated = NOW() \
             WHERE review_id = $1 AND status = 'PENDING_REVIEW'",
        )
        .bind(review_id.as_uuid())
        .bind(notes)
        .execute(&mut *transaction)
        .await?;
        release_work_waiting_on(&mut transaction, &review.waiting_work()).await?;
        commit_with_event(
            transaction,
            review_id,
            actor,
            ReviewMutation::Rejected { notes },
        )
        .await?;
        info!(review_id = %review_id, actor = %actor, "Review rejected");
        Ok(())
    }

    /// Records the needs-repair verdict on a pending review. The review stays pending, so its
    /// candidate stays editable, its pages and the pages waiting on it stay attached, and no
    /// second review opens for the same artifact; the reviewer later approves or rejects it. A
    /// repeated verdict replaces the note. A review agent's verdict is one round of the agents'
    /// back-and-forth; the one that reaches the cap is taken and leaves the review waiting for a
    /// human.
    pub async fn request_repair(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
        note: Option<&str>,
    ) -> Result<(), ReviewRepositoryError> {
        let note = note
            .map(str::trim)
            .filter(|note| !note.is_empty())
            .ok_or(ReviewRepositoryError::MissingRepairNote)?;
        let mut transaction = self.pool.begin().await?;
        lock_review_for_verdict(&mut transaction, review_id, actor, &self.agent_decisions).await?;
        sqlx::query(
            "UPDATE crawler_reviews \
             SET repair_requested = NOW(), repair_note = $2, updated = NOW() \
             WHERE review_id = $1 AND status = 'PENDING_REVIEW'",
        )
        .bind(review_id.as_uuid())
        .bind(note)
        .execute(&mut *transaction)
        .await?;
        commit_with_event(
            transaction,
            review_id,
            actor,
            ReviewMutation::RepairRequested { note },
        )
        .await?;
        info!(review_id = %review_id, actor = %actor, "Reviewer asked for a review to be repaired");
        Ok(())
    }
}

/// Refuses, before any review is read, a verdict the actor may not give on any review: the author
/// agent's, which only stages, and every agent verdict while agent decisions are off. Other actors
/// pass.
fn admit_agent_verdict(
    policy: &AgentDecisionPolicy,
    actor: ReviewActor,
) -> Result<(), ReviewRepositoryError> {
    if !actor.may_decide() {
        return Err(ReviewRepositoryError::AuthorMayNotDecide);
    }
    if actor.is_agent() && policy.approvals == AgentApprovals::Off {
        return Err(ReviewRepositoryError::AgentDecisionsOff);
    }
    Ok(())
}

/// As [`admit_agent_verdict`], and refuses an agent approval that acknowledges incomplete
/// coverage: only a human approves a candidate that does not cover every review page.
fn admit_approval(
    policy: &AgentDecisionPolicy,
    actor: ReviewActor,
    incomplete_coverage: IncompleteCoverage,
) -> Result<(), ReviewRepositoryError> {
    admit_agent_verdict(policy, actor)?;
    if actor.is_agent() && incomplete_coverage != IncompleteCoverage::Refuse {
        return Err(ReviewRepositoryError::AgentCoverageAcknowledgement);
    }
    Ok(())
}

/// As [`admit_agent_verdict`], and refuses an agent rejection without notes saying why.
fn admit_rejection(
    policy: &AgentDecisionPolicy,
    actor: ReviewActor,
    notes: Option<&str>,
) -> Result<(), ReviewRepositoryError> {
    admit_agent_verdict(policy, actor)?;
    if actor.is_agent() && notes.is_none_or(|notes| notes.trim().is_empty()) {
        return Err(ReviewRepositoryError::MissingRejectionNote);
    }
    Ok(())
}

/// Refuses an agent's approval of a review whose candidate or pages the same agent changed: no
/// agent approves its own work. Every event of a pending review records a candidate version up to
/// the one being approved, so each change it records is part of what the approval makes live. The
/// caller holds the review's row lock, so no change joins the trail meanwhile.
async fn refuse_approval_of_own_change(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    actor: ReviewActor,
    review: &ReviewUnderVerdict,
) -> Result<(), ReviewRepositoryError> {
    let changed_by_approver = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM crawler_review_events \
         WHERE review_id = $1 AND actor = $2 AND action = ANY($3) AND candidate_version <= $4)",
    )
    .bind(review_id.as_uuid())
    .bind(actor.as_str())
    .bind(ReviewEventAction::CHANGES_CANDIDATE.map(ReviewEventAction::as_str))
    .bind(review.candidate_version)
    .fetch_one(&mut **transaction)
    .await?;
    if changed_by_approver {
        return refuse_failed_checks(review_id, vec![FailedApprovalCheck::ApproverIsAuthor]);
    }
    Ok(())
}

/// Advisory lock key that serializes the review agents' verdicts for the rest of their
/// transactions, so two of them never both pass the hourly cap. Only agent verdicts take it, after
/// their review's row lock.
const AGENT_DECISION_LOCK: i64 = 0x2533_0001;

/// Refuses a review agent's verdict once the agents made `limit` approvals and rejections within
/// the last hour, as the event trail records them, so the cap holds across restarts.
async fn check_agent_decision_rate(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    limit: NonZeroU32,
) -> Result<(), ReviewRepositoryError> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(AGENT_DECISION_LOCK)
        .execute(&mut **transaction)
        .await?;
    let seconds_left = sqlx::query_scalar::<_, f64>(
        "SELECT EXTRACT(EPOCH FROM created + INTERVAL '1 hour' - clock_timestamp())::float8 \
         FROM crawler_review_events \
         WHERE actor = ANY($1) AND action = ANY($2) \
           AND created > clock_timestamp() - INTERVAL '1 hour' \
         ORDER BY created",
    )
    .bind(ReviewActor::AGENTS.map(ReviewActor::as_str))
    .bind([
        ReviewEventAction::Approved.as_str(),
        ReviewEventAction::Rejected.as_str(),
    ])
    .fetch_all(&mut **transaction)
    .await?;
    match agent_decision_wait(&seconds_left, limit) {
        None => Ok(()),
        Some(retry_after) => Err(ReviewRepositoryError::AgentDecisionLimitReached {
            limit: limit.get(),
            retry_after,
        }),
    }
}

/// How long the review agents wait before their next verdict is taken, given how many seconds each
/// of their verdicts of the last hour, oldest first, stays within that hour; `None` while they
/// made fewer than `limit`. Enough of the oldest must leave the hour to get below `limit`.
fn agent_decision_wait(seconds_left: &[f64], limit: NonZeroU32) -> Option<Duration> {
    let limit = usize::try_from(limit.get()).unwrap_or(usize::MAX);
    let last_to_leave = seconds_left.len().checked_sub(limit)?;
    Some(Duration::from_secs_f64(
        seconds_left[last_to_leave].ceil().max(1.0),
    ))
}

/// Refuses a review agent's approval of a product-schema review that fails a check of the
/// [approval guard](crate::review::approval_guard), against what approval would make live now
/// and the review's current pages.
fn guard_agent_product_schema_approval(
    review_id: CrawlerReviewId,
    review: &ReviewUnderVerdict,
    pages: &[CrawlerReviewPage],
    matrix: Option<&SchemaMatrix>,
    going_live: &ProductSchemaReviewCandidate,
    max_matrix_age: Duration,
) -> Result<(), ReviewRepositoryError> {
    let configuration_sha256 = going_live.fingerprint()?;
    refuse_failed_checks(
        review_id,
        product_schema_approval_checks(
            matrix,
            &CurrentReview {
                candidate_version: review.candidate_version,
                configuration_sha256: &configuration_sha256,
                pages,
            },
            OffsetDateTime::now_utc(),
            max_matrix_age,
        ),
    )
}

fn refuse_failed_checks(
    review_id: CrawlerReviewId,
    failed_checks: Vec<FailedApprovalCheck>,
) -> Result<(), ReviewRepositoryError> {
    if failed_checks.is_empty() {
        return Ok(());
    }
    Err(ReviewRepositoryError::AgentApprovalRefused {
        review_id,
        failed_checks,
    })
}

/// Marks the review approved and moves it past the approved candidate version, so an edit based
/// on the pending review is refused as stale instead of rewriting the live configuration, and
/// supersedes other pending reviews of the same artifact.
async fn close_approved_review(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    review: &ReviewUnderVerdict,
    approval: Approval<'_>,
) -> Result<(), ReviewRepositoryError> {
    // Storing a merged or rebased candidate already moved the version once; it moves once.
    sqlx::query(
        "UPDATE crawler_reviews \
         SET status = 'APPROVED', reviewer_notes = COALESCE($2, reviewer_notes), \
             candidate_version = $3 + 1, reviewed = NOW(), updated = NOW() \
         WHERE review_id = $1 AND status = 'PENDING_REVIEW'",
    )
    .bind(review_id.as_uuid())
    .bind(approval.notes)
    .bind(review.candidate_version)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE crawler_reviews \
         SET status = 'SUPERSEDED', reviewed = NOW(), updated = NOW() \
         WHERE listing_source_id = $1 AND artifact_type = $2 \
           AND status = 'PENDING_REVIEW' AND review_id <> $3 \
           AND ($4::uuid IS NULL OR domain_id = $4)",
    )
    .bind(uuid::Uuid::from(review.listing_source_id))
    .bind(&review.artifact_type)
    .bind(review_id.as_uuid())
    .bind(review.domain_id.map(CrawlerDomainId::into_uuid))
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Makes the reviewed decision of the URL-pattern review, its `candidate`, the live decision of
/// its domain inside the approval's transaction, classifies the domain's stored URLs with it by
/// `url_classifier`, and returns the decision it replaced and the one it made live.
async fn make_reviewed_url_pattern_live(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    review: &ReviewUnderVerdict,
    candidate: &UrlPatternReviewCandidate,
    url_classifier: UrlClassifier,
) -> Result<UrlPatternApprovalRecord, ReviewRepositoryError> {
    let pattern = candidate
        .validated_pattern()
        .map_err(|_| ReviewRepositoryError::InvalidUrlPatternCandidate)?;
    let domain_id = review.domain_id.ok_or(sqlx::Error::RowNotFound)?;
    let replaced = lock_domain_url_pattern(transaction, &review.listing_source_id, domain_id)
        .await?
        .ok_or(ReviewRepositoryError::NotFound(review_id))?;
    let approved = DomainUrlPattern {
        url_pattern: pattern.map(str::to_owned),
        url_pattern_state: if pattern.is_some() {
            "MATCHED"
        } else {
            "NO_PATTERN"
        }
        .to_owned(),
    };
    let reclassified = make_url_pattern_live(
        transaction,
        &review.listing_source_id,
        domain_id,
        &approved,
        url_classifier,
    )
    .await?;
    info!(
        review_id = %review_id,
        domain_id = %domain_id,
        reclassified_urls = reclassified,
        "Approved URL pattern classified the domain's stored URLs"
    );
    Ok(UrlPatternApprovalRecord {
        replaced_url_pattern: replaced,
        approved_url_pattern: approved,
    })
}

/// Locks and reads the domain's live URL-pattern decision, `None` when the domain is gone.
pub(super) async fn lock_domain_url_pattern(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    listing_source_id: &ListingSourceId,
    domain_id: CrawlerDomainId,
) -> Result<Option<DomainUrlPattern>, ReviewRepositoryError> {
    Ok(sqlx::query_as::<_, (Option<String>, String)>(
        "SELECT url_pattern, url_pattern_state FROM listing_source_domains \
         WHERE listing_source_id = $1 AND domain_id = $2 FOR UPDATE",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(domain_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await?
    .map(|(url_pattern, url_pattern_state)| DomainUrlPattern {
        url_pattern,
        url_pattern_state,
    }))
}

/// Stores `decision` as the domain's live URL-pattern decision and classifies the domain's
/// stored URLs with it by `url_classifier`, as the spider would on its next crawl. Returns how
/// many URLs changed class. Only a `MATCHED` decision classifies URLs as products.
pub(super) async fn make_url_pattern_live(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    listing_source_id: &ListingSourceId,
    domain_id: CrawlerDomainId,
    decision: &DomainUrlPattern,
    url_classifier: UrlClassifier,
) -> Result<u64, ReviewRepositoryError> {
    sqlx::query(
        "UPDATE listing_source_domains SET url_pattern = $3, url_pattern_state = $4 \
         WHERE listing_source_id = $1 AND domain_id = $2",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(domain_id.as_uuid())
    .bind(decision.url_pattern.as_deref())
    .bind(&decision.url_pattern_state)
    .execute(&mut **transaction)
    .await?;
    let live_pattern = decision
        .url_pattern
        .as_deref()
        .filter(|_| decision.url_pattern_state == "MATCHED")
        .map(Regex::new)
        .transpose()
        .map_err(|_| ReviewRepositoryError::InvalidUrlPatternCandidate)?;
    reclassify_domain_urls(
        transaction,
        listing_source_id,
        domain_id,
        live_pattern.as_ref(),
        url_classifier,
    )
    .await
}

/// The ListingSource's product-schema configuration an approval replaces and the one it makes
/// live, read under the locks of the live schema list and the stored removed-page templates,
/// which the approval holds until it commits.
struct ProductSchemasGoingLive {
    reviewed: ProductSchemaReviewCandidate,
    live: ProductSchemaReviewCandidate,
    /// The live schema list, `None` when none is live.
    replaced_product_schemas: Option<Vec<ProductCssSelectorSchema>>,
    replaced_removed_page_schemas: Vec<RemovedPageSchema>,
}

/// Locks the ListingSource's live schema list and stored removed-page templates and reads what
/// approving the review makes live in their place (see
/// [`ProductSchemaReviewCandidate::going_live`]). A stored list or base that no longer decodes
/// fails the approval.
async fn lock_product_schemas_going_live(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review: &ReviewUnderVerdict,
) -> Result<ProductSchemasGoingLive, ReviewRepositoryError> {
    let reviewed = ProductSchemaReviewCandidate::parse(&review.candidate_payload)?;
    let base: Vec<RemovedPageSchema> =
        serde_json::from_value(review.removed_page_schemas_base.clone())
            .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
    let existing = lock_live_product_schemas(transaction, &review.listing_source_id).await?;
    let stored = lock_stored_removed_page_schemas(transaction, &review.listing_source_id)
        .await?
        .unwrap_or_default();
    let live = reviewed.going_live(&review.reason, existing.as_ref(), &base, &stored)?;
    Ok(ProductSchemasGoingLive {
        reviewed,
        live,
        replaced_product_schemas: existing.map(|existing| existing.product_schemas),
        replaced_removed_page_schemas: stored,
    })
}

/// Makes `going_live` the ListingSource's live schema list and stored removed-page templates
/// inside the approval's transaction. The review keeps what went live, so edits through it are
/// not refused as stale. Returns what it replaced and what it made live.
async fn make_product_schemas_live(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    listing_source_id: &ListingSourceId,
    going_live: ProductSchemasGoingLive,
) -> Result<ProductSchemaApprovalRecord, ReviewRepositoryError> {
    let ProductSchemasGoingLive {
        reviewed,
        live,
        replaced_product_schemas,
        replaced_removed_page_schemas,
    } = going_live;
    persist_product_schemas(transaction, listing_source_id, &live.schemas).await?;
    if live.removed_page_schemas != replaced_removed_page_schemas {
        store_removed_page_schemas(transaction, listing_source_id, &live.removed_page_schemas)
            .await?;
        info!(
            review_id = %review_id,
            listing_source_id = %listing_source_id,
            removed_page_template_count = live.removed_page_schemas.len(),
            retired_template_count = replaced_removed_page_schemas
                .iter()
                .filter(|schema| !live.removed_page_schemas.contains(schema))
                .count(),
            "Approved removed-page templates from a product schema review"
        );
    }
    if live != reviewed {
        sqlx::query("UPDATE crawler_reviews SET candidate_payload = $2 WHERE review_id = $1")
            .bind(review_id.as_uuid())
            .bind(live.to_payload()?)
            .execute(&mut **transaction)
            .await?;
    }
    Ok(ProductSchemaApprovalRecord {
        replaced_product_schemas,
        replaced_removed_page_schemas,
        approved_product_schemas: live.schemas,
        approved_removed_page_schemas: live.removed_page_schemas,
    })
}

/// The state of a pending review a verdict decides on, read under the review's row lock.
struct ReviewUnderVerdict {
    listing_source_id: ListingSourceId,
    domain_id: Option<CrawlerDomainId>,
    artifact_type: String,
    reason: String,
    candidate_payload: serde_json::Value,
    candidate_version: i64,
    validation_summary: serde_json::Value,
    removed_page_schemas_base: serde_json::Value,
}

impl ReviewUnderVerdict {
    /// Refuses a verdict based on another candidate version than the review's, with the same
    /// refusal as a stale candidate edit.
    fn check_candidate_version(
        &self,
        review_id: CrawlerReviewId,
        expected: i64,
    ) -> Result<(), ReviewRepositoryError> {
        if self.candidate_version == expected {
            return Ok(());
        }
        Err(ReviewRepositoryError::StaleCandidateVersion {
            review_id,
            expected,
            current: self.candidate_version,
        })
    }

    fn waiting_work(&self) -> WaitingWork {
        WaitingWork {
            artifact_type: self.artifact_type.clone(),
            listing_source_id: self.listing_source_id,
            domain_id: self.domain_id,
        }
    }
}

/// Locks the review for `actor`'s verdict, so verdicts, edits and page joins on it apply one after
/// the other, and reads what the verdict decides on. Only a pending review takes a verdict; an
/// unknown review is told apart from a closed one. A review agent's verdict on a review that waits
/// for a human by `agent_decisions` is refused.
async fn lock_review_for_verdict(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    actor: ReviewActor,
    agent_decisions: &AgentDecisionPolicy,
) -> Result<ReviewUnderVerdict, ReviewRepositoryError> {
    let review = sqlx::query(
        "SELECT status, listing_source_id, domain_id, artifact_type, reason, candidate_payload, \
                candidate_version, validation_summary, removed_page_schemas_base \
         FROM crawler_reviews WHERE review_id = $1 FOR UPDATE",
    )
    .bind(review_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(ReviewRepositoryError::NotFound(review_id))?;
    let status: String = review.try_get("status")?;
    if status != STATUS_PENDING_REVIEW {
        return Err(ReviewRepositoryError::NotPending(review_id));
    }
    refuse_agent_while_human_required(transaction, review_id, actor, &status, agent_decisions)
        .await?;
    Ok(ReviewUnderVerdict {
        listing_source_id: listing_source_id_from_uuid(
            review.try_get::<uuid::Uuid, _>("listing_source_id")?,
        )?,
        domain_id: review
            .try_get::<Option<uuid::Uuid>, _>("domain_id")?
            .map(crawler_domain_id_from_uuid)
            .transpose()?,
        artifact_type: review.try_get("artifact_type")?,
        reason: review.try_get("reason")?,
        candidate_payload: review.try_get("candidate_payload")?,
        candidate_version: review.try_get("candidate_version")?,
        validation_summary: review.try_get("validation_summary")?,
        removed_page_schemas_base: review.try_get("removed_page_schemas_base")?,
    })
}

/// Refuses to approve a product-schema candidate whose stored schema `matrix` does not cover
/// every review page, unless the approver acknowledged exactly the review's current coverage.
/// The caller holds the review's row lock, which page joins and removals, candidate edits and
/// matrix stores take first too, so the coverage checked is the one the approval decides on. A
/// candidate change drops the stored matrix, so a stored matrix evaluated the current candidate;
/// one that no longer decodes vouches for no page. Returns the coverage the approval goes ahead
/// under.
fn check_approval_coverage(
    review_id: CrawlerReviewId,
    matrix: Option<&SchemaMatrix>,
    pages: &[CrawlerReviewPage],
    incomplete_coverage: IncompleteCoverage,
) -> Result<ReviewPageCoverage, ReviewRepositoryError> {
    let coverage = ReviewPageCoverage::of(matrix, pages);
    if !coverage.admits(incomplete_coverage) {
        return Err(match incomplete_coverage {
            IncompleteCoverage::Refuse => ReviewRepositoryError::IncompleteCoverage {
                review_id,
                coverage,
            },
            IncompleteCoverage::Acknowledged(acknowledged) => {
                ReviewRepositoryError::StaleCoverageAcknowledgement {
                    review_id,
                    coverage,
                    acknowledged,
                }
            }
        });
    }
    if !coverage.is_complete() {
        info!(
            review_id = %review_id,
            covered_pages = coverage.covered,
            removed_pages = coverage.removed,
            review_pages = coverage.total,
            not_evaluated_pages = coverage.not_evaluated,
            "Approving a product schema candidate with acknowledged incomplete coverage"
        );
    }
    Ok(coverage)
}

/// The work that waits on a review: the pages of its ListingSource that wait on its
/// product-schema review, or the domain whose crawl waits on its URL-pattern review.
pub(super) struct WaitingWork {
    pub(super) artifact_type: String,
    pub(super) listing_source_id: ListingSourceId,
    pub(super) domain_id: Option<CrawlerDomainId>,
}

/// Releases the work that waited on a decided review for an immediate retry: every page of the
/// ListingSource that waits on its product-schema review, or the domain whose crawl waits on its
/// URL-pattern review. A verdict runs it last in its transaction, after the review row is locked
/// and closed, so a wait written while the verdict commits waits for it and then finds the review
/// closed instead of holding back a page the verdict released; a restore releases the same work
/// once the configuration it brought back is stored.
pub(super) async fn release_work_waiting_on(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review: &WaitingWork,
) -> Result<(), ReviewRepositoryError> {
    match review.artifact_type.as_str() {
        ARTIFACT_PRODUCT_SCHEMA => {
            sqlx::query(
                "UPDATE listing_source_urls \
                 SET next_retry_at = NULL, last_error_kind = NULL, \
                     last_error_message = NULL, schema_review_waited_on = NULL, \
                     schema_review_wait_streak = 0, updated = NOW() \
                 WHERE listing_source_id = $1 \
                   AND last_error_kind = 'PendingSchemaReview' \
                   AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')",
            )
            .bind(uuid::Uuid::from(review.listing_source_id))
            .execute(&mut **transaction)
            .await?;
        }
        ARTIFACT_URL_PATTERN => {
            if let Some(domain_id) = review.domain_id {
                release_domain_waiting_for_url_pattern(
                    transaction,
                    &review.listing_source_id,
                    domain_id,
                )
                .await?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Classifies the domain's stored URLs with the pattern decision that just went live, as the
/// spider would on its next crawl, so URLs a crawl stored while the review was pending need no
/// second crawl from the root. Sold URLs keep their class, as a re-crawl leaves them.
async fn reclassify_domain_urls(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    listing_source_id: &ListingSourceId,
    domain_id: CrawlerDomainId,
    pattern: Option<&Regex>,
    url_classifier: UrlClassifier,
) -> Result<u64, ReviewRepositoryError> {
    let stored = sqlx::query_as::<_, (String, String)>(
        "SELECT url, url_class FROM listing_source_urls \
         WHERE listing_source_id = $1 AND domain_id = $2 AND crawler_disposition = 'ACTIVE'",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(domain_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await?;
    let (urls, classes): (Vec<String>, Vec<String>) = stored
        .into_iter()
        .filter_map(|(url, current_class)| {
            let class = url_classifier.classify(&url, pattern).to_string();
            (class != current_class).then_some((url, class))
        })
        .unzip();
    if urls.is_empty() {
        return Ok(0);
    }
    let result = sqlx::query(
        "UPDATE listing_source_urls AS u \
         SET url_class = input.url_class, updated = NOW() \
         FROM UNNEST($1::text[], $2::text[]) AS input(url, url_class) \
         WHERE u.url = input.url AND u.listing_source_id = $3 AND u.domain_id = $4",
    )
    .bind(&urls)
    .bind(&classes)
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(domain_id.as_uuid())
    .execute(&mut **transaction)
    .await?;
    Ok(result.rows_affected())
}

/// Lets the scheduler crawl a domain again whose last crawl stopped at a URL-pattern review,
/// now that the review is decided. Rows written before the review-wait outcome existed carry
/// the wait as `last_crawl_error_kind` instead and are released the same way.
async fn release_domain_waiting_for_url_pattern(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    listing_source_id: &ListingSourceId,
    domain_id: CrawlerDomainId,
) -> Result<(), ReviewRepositoryError> {
    sqlx::query(
        "UPDATE listing_source_domains \
         SET next_crawl_at = NULL, \
             last_crawl_error_kind = CASE \
                 WHEN last_crawl_error_kind = 'PendingUrlPatternReview' THEN NULL \
                 ELSE last_crawl_error_kind \
             END \
         WHERE listing_source_id = $1 AND domain_id = $2 \
           AND (last_crawl_error_kind = 'PendingUrlPatternReview' OR last_crawl_outcome = $3)",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(domain_id.as_uuid())
    .bind(AWAITING_URL_PATTERN_REVIEW_CRAWL_OUTCOME)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::page_coverage::ReviewPageCoverage;

    fn policy(approvals: AgentApprovals) -> AgentDecisionPolicy {
        AgentDecisionPolicy {
            approvals,
            ..AgentDecisionPolicy::default()
        }
    }

    const ACKNOWLEDGED: IncompleteCoverage = IncompleteCoverage::Acknowledged(ReviewPageCoverage {
        covered: 1,
        removed: 0,
        total: 2,
        not_evaluated: 0,
    });

    /// With agent decisions off a human decides; humans and the crawler decide as before in both
    /// modes.
    #[test]
    fn should_refuse_every_agent_verdict_only_while_agent_decisions_are_off() {
        let off = policy(AgentApprovals::Off);
        let guarded = policy(AgentApprovals::Guarded);

        assert!(matches!(
            admit_approval(&off, ReviewActor::AgentReviewer, IncompleteCoverage::Refuse),
            Err(ReviewRepositoryError::AgentDecisionsOff)
        ));
        assert!(matches!(
            admit_rejection(&off, ReviewActor::AgentReviewer, Some("not a product page")),
            Err(ReviewRepositoryError::AgentDecisionsOff)
        ));
        assert!(
            admit_approval(
                &guarded,
                ReviewActor::AgentReviewer,
                IncompleteCoverage::Refuse
            )
            .is_ok()
        );
        assert!(
            admit_rejection(
                &guarded,
                ReviewActor::AgentReviewer,
                Some("not a product page")
            )
            .is_ok()
        );
        for actor in [ReviewActor::Human, ReviewActor::System] {
            for policy in [&off, &guarded] {
                assert!(
                    admit_approval(policy, actor, ACKNOWLEDGED).is_ok(),
                    "{actor}"
                );
                assert!(admit_rejection(policy, actor, None).is_ok(), "{actor}");
            }
        }
    }

    /// The author agent stages and never decides, whatever the policy.
    #[test]
    fn should_refuse_every_verdict_of_an_agent_that_does_not_decide() {
        let actor = ReviewActor::AgentAuthor;
        for policy in [policy(AgentApprovals::Off), policy(AgentApprovals::Guarded)] {
            assert!(matches!(
                admit_approval(&policy, actor, IncompleteCoverage::Refuse),
                Err(ReviewRepositoryError::AuthorMayNotDecide)
            ));
            assert!(matches!(
                admit_rejection(&policy, actor, Some("not a product page")),
                Err(ReviewRepositoryError::AuthorMayNotDecide)
            ));
        }
    }

    #[test]
    fn should_refuse_an_agent_approval_that_acknowledges_coverage_and_a_rejection_without_notes() {
        let guarded = policy(AgentApprovals::Guarded);

        assert!(matches!(
            admit_approval(&guarded, ReviewActor::AgentReviewer, ACKNOWLEDGED),
            Err(ReviewRepositoryError::AgentCoverageAcknowledgement)
        ));
        for notes in [None, Some(""), Some(" \n ")] {
            assert!(
                matches!(
                    admit_rejection(&guarded, ReviewActor::AgentReviewer, notes),
                    Err(ReviewRepositoryError::MissingRejectionNote)
                ),
                "{notes:?}"
            );
        }
    }

    /// Below the cap the agent decides at once; at it, it waits until enough of its oldest
    /// decisions of the hour leave the hour to get below the cap again.
    #[test]
    fn should_wait_until_enough_agent_decisions_leave_the_hour() {
        let cap = NonZeroU32::new(3).unwrap_or(NonZeroU32::MIN);

        assert_eq!(agent_decision_wait(&[], cap), None);
        assert_eq!(agent_decision_wait(&[10.0, 20.0], cap), None);
        assert_eq!(
            agent_decision_wait(&[10.2, 20.0, 30.0], cap),
            Some(Duration::from_secs(11))
        );
        assert_eq!(
            agent_decision_wait(&[10.0, 20.0, 30.0, 40.0, 50.0], cap),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            agent_decision_wait(&[0.2, 20.0, 30.0], cap),
            Some(Duration::from_secs(1)),
            "a decision about to leave the hour still waits a second"
        );
    }
}
