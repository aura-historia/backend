//! The review agents' needs-repair rounds on a review: every needs-repair verdict an agent gave it
//! since a human last changed or judged it, counted from its event trail. Once a pending review
//! reached the cap of
//! [`AgentLimits::max_repair_rounds`](crate::review::agent_decisions::AgentLimits::max_repair_rounds)
//! it waits for a human, and every agent change and verdict on it is refused until a human's own
//! change or verdict hands it back and the count restarts. The contract is in
//! `docs/crawler/operations.md` (review agents).

use super::{ReviewRepositoryError, crawler_review_id_from_uuid};
use crate::CrawlerReviewId;
use crate::review::agent_decisions::AgentDecisionPolicy;
use crate::review::model::{ReviewActor, ReviewEventAction, STATUS_PENDING_REVIEW};
use std::collections::HashMap;

/// The agents' needs-repair rounds of the review aliased `r`: its agent `REPAIR_REQUESTED` events
/// after its latest `HUMAN` event, or after its first event when no human acted on it. A review's
/// events append under its row lock in commit order, each at the time of its write, so their times
/// order them. Every query that counts the rounds uses this expression, so their counts cannot
/// drift. Binds `$1`, the agent actors ([`agent_round_binds`]), `$2`, the needs-repair action, and
/// `$3`, the human actor.
macro_rules! agent_rounds_of_review {
    () => {
        "(SELECT COUNT(*) FROM crawler_review_events e \
          WHERE e.review_id = r.review_id AND e.actor = ANY($1) AND e.action = $2 \
            AND e.created > COALESCE( \
                (SELECT MAX(h.created) FROM crawler_review_events h \
                 WHERE h.review_id = r.review_id AND h.actor = $3), \
                '-infinity'::timestamptz))"
    };
}
pub(super) use agent_rounds_of_review;

/// The values of `$1` to `$3` of [`agent_rounds_of_review`]: the agent actors, the needs-repair
/// action and the human actor.
pub(super) fn agent_round_binds() -> ([&'static str; 2], &'static str, &'static str) {
    (
        ReviewActor::AGENTS.map(ReviewActor::as_str),
        ReviewEventAction::RepairRequested.as_str(),
        ReviewActor::Human.as_str(),
    )
}

/// The agents' needs-repair rounds of each of `review_ids`, as [`agent_rounds_of_review`] counts
/// them.
pub(super) async fn agent_repair_rounds<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    review_ids: &[CrawlerReviewId],
) -> Result<HashMap<CrawlerReviewId, u32>, sqlx::Error> {
    let review_ids: Vec<uuid::Uuid> = review_ids.iter().map(|id| *id.as_uuid()).collect();
    let (agents, repair_requested, human) = agent_round_binds();
    sqlx::query_as::<_, (uuid::Uuid, i64)>(concat!(
        "SELECT r.review_id, ",
        agent_rounds_of_review!(),
        " FROM crawler_reviews r WHERE r.review_id = ANY($4)"
    ))
    .bind(agents)
    .bind(repair_requested)
    .bind(human)
    .bind(&review_ids)
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(|(review_id, rounds)| {
        // A count beyond `u32` is past any cap, so saturating keeps the review with a human.
        Ok((
            crawler_review_id_from_uuid(review_id)?,
            u32::try_from(rounds).unwrap_or(u32::MAX),
        ))
    })
    .collect()
}

/// Refuses a review agent's change or verdict on the review `review_id` while it is pending
/// (`status`) and waits for a human; any other actor, and any agent on a review that does not wait,
/// passes. The caller holds the review's row lock, which every mutation of the review holds while it
/// appends its event, so the rounds counted are those the change would follow and no agent request
/// racing the verdict that reached the cap gets past it.
pub(super) async fn refuse_agent_while_human_required(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    actor: ReviewActor,
    status: &str,
    agent_decisions: &AgentDecisionPolicy,
) -> Result<(), ReviewRepositoryError> {
    if !actor.is_agent() || status != STATUS_PENDING_REVIEW {
        return Ok(());
    }
    let agent_rounds = agent_repair_rounds(&mut **transaction, &[review_id])
        .await?
        .remove(&review_id)
        .unwrap_or(0);
    if agent_decisions.waits_for_human(status, agent_rounds) {
        return Err(ReviewRepositoryError::HumanRequired {
            review_id,
            agent_rounds,
            max_repair_rounds: agent_decisions.limits().max_repair_rounds.get(),
        });
    }
    Ok(())
}
