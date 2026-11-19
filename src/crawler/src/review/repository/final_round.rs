//! The review agents' final round: once a pending review would wait for a human, because their
//! needs-repair rounds reached the cap or a review was handed over, the author agent starts one
//! more round with the strongest model before the review's ListingSource is parked. Starting it
//! appends a `FINAL_ROUND_STARTED` event, from which the review is the agents' again until a
//! needs-repair verdict or a hand-over ends the round ([`FinalRound`]). Approvals in it pass the
//! unchanged approval guard and four-eyes rule. A review gets it at most once within the round
//! window since a human last acted on it, and starting it counts towards the agents' hourly
//! decision cap. The contract is in `docs/crawler/operations.md` (review agents).

use super::repair_rounds::rounds_under_lock;
use super::review_events::{ReviewMutation, commit_with_event};
use super::verdicts::check_agent_decision_rate;
use super::{CrawlerReviewRepository, ReviewRepositoryError};
use crate::CrawlerReviewId;
#[cfg(doc)]
use crate::review::agent_decisions::FinalRound;
use crate::review::model::{FinalRoundCause, ReviewActor, STATUS_PENDING_REVIEW};

impl CrawlerReviewRepository {
    /// Starts the agents' final round on the review `review_id` as `actor`, which must be the author
    /// agent: the review must be pending and [due](crate::review::agent_decisions::AgentDecisionPolicy::final_round_due)
    /// for it, and the agents' hourly decision cap must have room.
    pub async fn start_final_round(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
    ) -> Result<FinalRoundCause, ReviewRepositoryError> {
        match actor {
            ReviewActor::AgentAuthor => {}
            ReviewActor::AgentReviewer => {
                return Err(ReviewRepositoryError::ReviewerMayNotChangeCandidates);
            }
            ReviewActor::Human | ReviewActor::System => {
                return Err(ReviewRepositoryError::FinalRoundNeedsAuthorAgent);
            }
        }
        let mut transaction = self.pool.begin().await?;
        let status = sqlx::query_scalar::<_, String>(
            "SELECT status FROM crawler_reviews WHERE review_id = $1 FOR UPDATE",
        )
        .bind(review_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(ReviewRepositoryError::NotFound(review_id))?;
        if status != STATUS_PENDING_REVIEW {
            return Err(ReviewRepositoryError::NotPending(review_id));
        }
        let rounds = rounds_under_lock(&mut transaction, review_id, &self.agent_decisions).await?;
        if !self.agent_decisions.final_round_due(&status, rounds) {
            return Err(ReviewRepositoryError::FinalRoundNotDue(review_id));
        }
        check_agent_decision_rate(
            &mut transaction,
            self.agent_decisions.limits().max_decisions_per_hour,
        )
        .await?;
        let after = if rounds.handed_to_human {
            FinalRoundCause::HandedOver
        } else {
            FinalRoundCause::RepairCap
        };
        sqlx::query("UPDATE crawler_reviews SET updated = NOW() WHERE review_id = $1")
            .bind(review_id.as_uuid())
            .execute(&mut *transaction)
            .await?;
        commit_with_event(
            transaction,
            review_id,
            actor,
            ReviewMutation::FinalRoundStarted { after },
        )
        .await?;
        Ok(after)
    }
}
