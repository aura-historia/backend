//! The review agents' needs-repair rounds on a review: every needs-repair verdict an agent gave it
//! since a human last changed or judged it, counted from its event trail. Once a pending review
//! reached the cap of
//! [`AgentLimits::max_repair_rounds`](crate::review::agent_decisions::AgentLimits::max_repair_rounds)
//! it waits for a human, and every agent change and verdict on it is refused until a human's own
//! change or verdict hands it back and the count restarts. The contract is in
//! `docs/crawler/operations.md` (review agents).

use super::{ReviewRepositoryError, crawler_review_id_from_uuid};
use crate::CrawlerReviewId;
use crate::review::agent_decisions::{AgentDecisionPolicy, AgentRounds};
use crate::review::model::{ReviewActor, ReviewEventAction, STATUS_PENDING_REVIEW};
use std::collections::HashMap;

/// The part of a review's event trail that follows its latest `HUMAN` event, or all of it when no
/// human acted on it, and that lies within the round window: a `WHERE` condition on the event `e`
/// of the review aliased `r`. Binds `$3`, the human actor, and `$4`, the window in days or null for
/// none ([`AgentDecisionPolicy::repair_round_window_days`]).
macro_rules! after_last_human_event {
    () => {
        "e.created > GREATEST( \
            COALESCE( \
                (SELECT MAX(h.created) FROM crawler_review_events h \
                 WHERE h.review_id = r.review_id AND h.actor = $3), \
                '-infinity'::timestamptz), \
            COALESCE(NOW() - make_interval(days => $4::int), '-infinity'::timestamptz))"
    };
}
pub(crate) use after_last_human_event;

/// The agents' needs-repair rounds of the review aliased `r`: its agent `REPAIR_REQUESTED` events
/// after its latest `HUMAN` event, or after its first event when no human acted on it, and within
/// the round window, so a review no human answered returns to the agents once its rounds age out. A review's
/// events append under its row lock in commit order, each at the time of its write, so their times
/// order them. Every query that counts the rounds uses this expression, so their counts cannot
/// drift. Binds `$1`, the agent actors ([`agent_round_binds`]), `$2`, the needs-repair action,
/// `$3`, the human actor, and `$4`, the round window in days or null.
macro_rules! agent_rounds_of_review {
    () => {
        concat!(
            "(SELECT COUNT(*) FROM crawler_review_events e \
              WHERE e.review_id = r.review_id AND e.actor = ANY($1) AND e.action = $2 AND ",
            after_last_human_event!(),
            ")"
        )
    };
}
pub(crate) use agent_rounds_of_review;

/// Whether an agent handed the review aliased `r` to a human since a human last acted on it: it
/// waits for a human whatever its rounds, until the hand-over leaves the round window. Binds `$1`,
/// `$3` and `$4` as [`agent_rounds_of_review`].
macro_rules! agent_handed_review_to_human {
    () => {
        concat!(
            "EXISTS (SELECT 1 FROM crawler_review_events e \
              WHERE e.review_id = r.review_id AND e.actor = ANY($1) \
                AND e.action = 'HANDED_TO_HUMAN' AND ",
            after_last_human_event!(),
            ")"
        )
    };
}
pub(crate) use agent_handed_review_to_human;

/// The values of `$1` to `$4` of [`agent_rounds_of_review`] under `agent_decisions`: the agent
/// actors, the needs-repair action, the human actor and the round window in days.
pub(crate) fn agent_round_binds(
    agent_decisions: &AgentDecisionPolicy,
) -> ([&'static str; 2], &'static str, &'static str, Option<i32>) {
    (
        ReviewActor::AGENTS.map(ReviewActor::as_str),
        ReviewEventAction::RepairRequested.as_str(),
        ReviewActor::Human.as_str(),
        agent_decisions.repair_round_window_days(),
    )
}

/// What the agents did on each of `review_ids` since a human last acted on it and within the round
/// window of `agent_decisions`, as [`agent_rounds_of_review`] and [`agent_handed_review_to_human`]
/// read the trail.
pub(crate) async fn agent_repair_rounds<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    agent_decisions: &AgentDecisionPolicy,
    review_ids: &[CrawlerReviewId],
) -> Result<HashMap<CrawlerReviewId, AgentRounds>, sqlx::Error> {
    let review_ids: Vec<uuid::Uuid> = review_ids.iter().map(|id| *id.as_uuid()).collect();
    let (agents, repair_requested, human, window_days) = agent_round_binds(agent_decisions);
    sqlx::query_as::<_, (uuid::Uuid, i64, bool)>(concat!(
        "SELECT r.review_id, ",
        agent_rounds_of_review!(),
        ", ",
        agent_handed_review_to_human!(),
        " FROM crawler_reviews r WHERE r.review_id = ANY($5)"
    ))
    .bind(agents)
    .bind(repair_requested)
    .bind(human)
    .bind(window_days)
    .bind(&review_ids)
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(|(review_id, rounds, handed_to_human)| {
        // A count beyond `u32` is past any cap, so saturating keeps the review with a human.
        Ok((
            crawler_review_id_from_uuid(review_id)?,
            AgentRounds {
                needs_repair: u32::try_from(rounds).unwrap_or(u32::MAX),
                handed_to_human,
            },
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
    let rounds = agent_repair_rounds(&mut **transaction, agent_decisions, &[review_id])
        .await?
        .remove(&review_id)
        .unwrap_or_default();
    if agent_decisions.waits_for_human(status, rounds) {
        return Err(ReviewRepositoryError::HumanRequired {
            review_id,
            agent_rounds: rounds.needs_repair,
            handed_to_human: rounds.handed_to_human,
            max_repair_rounds: agent_decisions.limits().max_repair_rounds.get(),
        });
    }
    Ok(())
}
