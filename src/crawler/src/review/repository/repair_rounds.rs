//! The review agents' needs-repair rounds on a review: every needs-repair verdict an agent gave it
//! since a human last changed or judged it, counted from its event trail. Once a pending review
//! reached the cap of
//! [`AgentLimits::max_repair_rounds`](crate::review::agent_decisions::AgentLimits::max_repair_rounds)
//! it waits for a human, and every agent change and verdict on it is refused until a human's own
//! change or verdict hands it back and the count restarts. The contract is in
//! `docs/crawler/operations.md` (review agents).

use super::{ReviewRepositoryError, crawler_review_id_from_uuid};
use crate::CrawlerReviewId;
use crate::review::agent_decisions::{AgentDecisionPolicy, AgentRounds, FinalRound};
use crate::review::model::{ReviewActor, ReviewEventAction, STATUS_PENDING_REVIEW};
use std::collections::HashMap;

/// The part of a review's event trail that follows the latest act of a human on it, or all of it
/// when no human acted, and that lies within the round window: a `WHERE` condition on the event
/// `e` of the review aliased `r`. A human acts on a review with an event of its own, and on every
/// review of its ListingSource by labelling the source, clearing its label or deciding or clearing
/// its qualification, which hands the source's reviews back to the agents' regular rounds, so no
/// final round or park outlives it. Binds `$3`, the human actor, and `$4`, the window in days or
/// null for none ([`AgentDecisionPolicy::repair_round_window_days`]).
macro_rules! after_last_human_event {
    () => {
        "e.created > GREATEST( \
            COALESCE( \
                (SELECT MAX(h.created) FROM crawler_review_events h \
                 WHERE h.review_id = r.review_id AND h.actor = $3), \
                '-infinity'::timestamptz), \
            COALESCE( \
                (SELECT MAX(l.occurred_at) FROM listing_source_label_events l \
                 WHERE l.listing_source_id = r.listing_source_id AND l.actor = $3), \
                '-infinity'::timestamptz), \
            COALESCE( \
                (SELECT MAX(q.occurred_at) FROM listing_source_qualification_events q \
                 WHERE q.listing_source_id = r.listing_source_id AND q.actor = $3), \
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
            $crate::review::repository::after_last_human_event!(),
            ")"
        )
    };
}
pub(crate) use agent_rounds_of_review;

/// Whether the review aliased `r` was handed to a human since a human last acted on it: by an
/// agent, until the hand-over leaves the round window, or by the crawler's non-product loop guard
/// ([`held_for_non_product_loop`](super::held_for_non_product_loop)), which does
/// not lapse. It then waits for a human whatever its rounds. Binds `$1`, `$3` and `$4` as
/// [`agent_rounds_of_review`].
macro_rules! handed_review_to_human {
    () => {
        concat!(
            "(EXISTS (SELECT 1 FROM crawler_review_events e \
              WHERE e.review_id = r.review_id AND e.actor = ANY($1) \
                AND e.action = 'HANDED_TO_HUMAN' AND ",
            $crate::review::repository::after_last_human_event!(),
            ") OR ",
            $crate::review::repository::held_for_non_product_loop!(),
            ")"
        )
    };
}
pub(crate) use handed_review_to_human;

/// When the agents' final round on the review aliased `r` started: its latest agent
/// `FINAL_ROUND_STARTED` event after its latest `HUMAN` event and within the round window, null when
/// none. Binds `$1`, `$3` and `$4` as [`agent_rounds_of_review`].
macro_rules! final_round_started_at {
    () => {
        concat!(
            "(SELECT MAX(e.created) FROM crawler_review_events e \
              WHERE e.review_id = r.review_id AND e.actor = ANY($1) \
                AND e.action = 'FINAL_ROUND_STARTED' AND ",
            $crate::review::repository::after_last_human_event!(),
            ")"
        )
    };
}

/// Where the agents' final round on the review aliased `r` stands, as the code of
/// [`FinalRound::from_code`](crate::review::agent_decisions::FinalRound::from_code): `0` not started,
/// `1` open, `2` ended by an agent's hand-over after it started, or by the agents' needs-repair
/// verdict after the round's
/// [repair passes](crate::review::agent_decisions::FINAL_ROUND_REPAIR_PASSES), the `1` the count
/// is compared with. Binds `$1`, `$3` and `$4` as [`agent_rounds_of_review`].
macro_rules! final_round_of_review {
    () => {
        concat!(
            "(CASE WHEN ",
            $crate::review::repository::final_round_started_at!(),
            " IS NULL THEN 0 \
               WHEN EXISTS (SELECT 1 FROM crawler_review_events f \
                    WHERE f.review_id = r.review_id AND f.actor = ANY($1) \
                      AND f.action = 'HANDED_TO_HUMAN' AND f.created > ",
            $crate::review::repository::final_round_started_at!(),
            ") OR (SELECT COUNT(*) FROM crawler_review_events f \
                    WHERE f.review_id = r.review_id AND f.actor = ANY($1) \
                      AND f.action = 'REPAIR_REQUESTED' AND f.created > ",
            $crate::review::repository::final_round_started_at!(),
            ") > 1 THEN 2 ELSE 1 END)"
        )
    };
}
pub(crate) use {final_round_of_review, final_round_started_at};

/// Whether the pending review aliased `r` waits for a human: where the review agents work the
/// reviews (`$5`, the repair-round cap, is set), their rounds on it reached the cap or it was
/// handed to a human ([`handed_review_to_human`]) and no final round started since, or the
/// agents' final round on it ended ([`final_round_of_review`]). The SQL counterpart of
/// [`AgentDecisionPolicy::waits_for_human`] for a pending review, which every query that selects or
/// counts the reviews that wait for a human uses, so their answers cannot drift. Binds `$1` to `$4`
/// as [`agent_rounds_of_review`] and `$5`, the repair-round cap or null.
macro_rules! review_waits_for_human {
    () => {
        concat!(
            "($5::bigint IS NOT NULL AND CASE ",
            $crate::review::repository::final_round_of_review!(),
            " WHEN 0 THEN (",
            $crate::review::repository::agent_rounds_of_review!(),
            " >= $5::bigint OR ",
            $crate::review::repository::handed_review_to_human!(),
            ") WHEN 1 THEN FALSE ELSE TRUE END)"
        )
    };
}
pub(crate) use review_waits_for_human;

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
/// window of `agent_decisions`, as [`agent_rounds_of_review`], [`handed_review_to_human`] and
/// [`final_round_of_review`] read the trail.
pub(crate) async fn agent_repair_rounds<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    agent_decisions: &AgentDecisionPolicy,
    review_ids: &[CrawlerReviewId],
) -> Result<HashMap<CrawlerReviewId, AgentRounds>, sqlx::Error> {
    let review_ids: Vec<uuid::Uuid> = review_ids.iter().map(|id| *id.as_uuid()).collect();
    let (agents, repair_requested, human, window_days) = agent_round_binds(agent_decisions);
    sqlx::query_as::<_, (uuid::Uuid, i64, bool, i32)>(concat!(
        "SELECT r.review_id, ",
        agent_rounds_of_review!(),
        ", ",
        handed_review_to_human!(),
        ", ",
        final_round_of_review!(),
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
    .map(|(review_id, rounds, handed_to_human, final_round)| {
        // A count beyond `u32` is past any cap, so saturating keeps the review with a human.
        Ok((
            crawler_review_id_from_uuid(review_id)?,
            AgentRounds {
                needs_repair: u32::try_from(rounds).unwrap_or(u32::MAX),
                handed_to_human,
                final_round: FinalRound::from_code(final_round),
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
    let rounds = rounds_under_lock(transaction, review_id, agent_decisions).await?;
    if agent_decisions.waits_for_human(status, rounds) {
        return Err(human_required(review_id, rounds, agent_decisions));
    }
    Ok(())
}

/// What the agents did on the review `review_id`, whose row lock the caller holds.
pub(super) async fn rounds_under_lock(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    agent_decisions: &AgentDecisionPolicy,
) -> Result<AgentRounds, sqlx::Error> {
    Ok(
        agent_repair_rounds(&mut **transaction, agent_decisions, &[review_id])
            .await?
            .remove(&review_id)
            .unwrap_or_default(),
    )
}

/// The refusal of an agent's change of the review `review_id`, which waits for a human after
/// `rounds`.
pub(super) fn human_required(
    review_id: CrawlerReviewId,
    rounds: AgentRounds,
    agent_decisions: &AgentDecisionPolicy,
) -> ReviewRepositoryError {
    ReviewRepositoryError::HumanRequired {
        review_id,
        agent_rounds: rounds.needs_repair,
        handed_to_human: rounds.handed_to_human,
        max_repair_rounds: agent_decisions.limits().max_repair_rounds.get(),
    }
}
