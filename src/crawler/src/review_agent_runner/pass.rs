//! One pass of the agents over the pending reviews: per review a fresh author process, then, when
//! the author staged or kept a candidate, a fresh reviewer process, with at most the configured
//! number of processes at once. The pass is a rolling pool: a freed slot is refilled from the
//! queue as it is then, until the run's budget of reviews is spent, each review worked once.
//! Each process's outcome is recorded in the ledger and logged.
//! A process that ends without a decision cools its review down, and a usage limit pauses the
//! runner until its reset. Whether a reviewer decided is what the review's event trail records,
//! not what its report says the console answered. A review that waits for a human is no agent
//! work: the pass starts no process for it, nor a further process once an agent's pass handed it
//! to a human, so it neither fails nor cools down, whichever process's needs-repair verdict
//! brought it to the cap. Once a human hands it back it is workable at once, and the pass picks it
//! as a review no agent worked ([`select_reviews`]).
//! An author that reports `NOTHING_TO_STAGE` on a candidate the reviewer already judged, with no
//! page joined since, gives the reviewer nothing new, and one that reports `NEEDS_HUMAN` cannot
//! decide the review from what it can read: the pass hands the review to a human at once,
//! with the author's reasoning as the note of the console's hand-over event, instead of one more
//! needs-repair round.
//! Once the runner stops, the pass starts no process and stops the running ones, which it records
//! with their cost; their reviews do not cool down.

use super::context_use::ContextUse;
use super::decision_cap::{
    CLOCK_MARGIN, DECISION_WINDOW, decision_cap_resumes_at, log_reviewer_pause_ended,
    report_shows_decision_cap,
};
use super::digest::timestamp;
use super::envelope::{EnvelopeOutcome, ResultEnvelope, UsageLimit, UsageLimitKind};
use super::invocation::{agent_command, role_contract};
use super::ledger::{ProcessEnd, ProcessLedger, ProcessRecord};
use super::process::{ChildError, ChildOutput, ChildStop, run_agent};
use super::queue::{
    QueuedReview, ReviewArtifact, approval_failures, hand_to_human, review_events_since,
    review_queue, reviewer_judged_review, select_reviews, waits_for_human,
};
use super::reports::{AgentReport, AuthorAction, AuthorReport, ReviewerDecision, TrailVerdict};
use super::settings::{RunnerSettings, effort_flag};
use super::state::{Cooldown, Pause, ReviewerPause, RunnerState, save_state};
use super::stop::RunnerStop;
use super::usage_pacing::UsageWindows;
use super::wire_name as name;
use crate::CrawlerReviewId;
use crate::review::agent_decisions::AgentLimits;
use crate::review::model::{ReviewActor, ReviewEventAction};
use crate::review_mcp::{AgentRole, ConsoleClient, ConsoleError};
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use std::collections::BTreeSet;
use std::sync::{Mutex, PoisonError};
use time::OffsetDateTime;
use tracing::{debug, info, warn};

const PASS_STARTED_EVENT: &str = "crawler.review_agent.pass_started";
const PROCESS_FINISHED_EVENT: &str = "crawler.review_agent.process_finished";

/// What a pass did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PassSummary {
    /// Pending reviews the agents could work.
    pub pending: usize,
    /// Pending reviews that wait for a human, which no process works.
    pub waiting_for_human: usize,
    /// Of the workable ones, reviews still cooling down.
    pub cooling_down: usize,
    /// Reviews this pass started to work.
    pub worked: usize,
    pub processes: usize,
    /// Reviews the server took a reviewer decision on.
    pub decided: usize,
    /// Reviews this pass worked that waited for a human after the author's pass, so no reviewer
    /// process started for them.
    pub handed_to_human: usize,
    /// Reviews this pass worked whose reviewer process did not start because the reviewers pause
    /// after the agents' decision cap.
    pub reviewers_held_back: usize,
    /// URL-pattern reviews whose reviewer process did not start because the approval guard
    /// already refuses the candidate the author left: the reviewer could only refuse it again.
    pub guard_blocked: usize,
    /// Why the pass started no or no further process, if it stopped early.
    pub stopped: Option<PassStop>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PassStop {
    /// A usage limit of the CLI's login pauses the runner until then.
    Paused(OffsetDateTime),
    /// The runner stops.
    RunnerStop,
}

/// What a pass works with and changes: the runner's state and ledger.
struct Pass<'a> {
    settings: &'a RunnerSettings,
    console: &'a ConsoleClient,
    /// The limits of the agents' decisions the console applies, which the prompts tell them.
    limits: AgentLimits,
    runner_stop: &'a RunnerStop,
    shared: Mutex<Shared>,
}

struct Shared {
    state: RunnerState,
    ledger: ProcessLedger,
    summary: PassSummary,
    ledger_error: Option<super::ledger::LedgerError>,
}

/// How one agent process of a review ended, for the review's next step.
enum Step {
    /// The pass started no process: it is paused or the runner stops.
    NotStarted,
    Reported(AgentReport),
    /// It ended without a decision; its review cools down.
    NoDecision,
    /// The CLI's login ran into a usage limit; the runner pauses.
    UsageLimit,
    /// The runner's stop stopped it; its review neither goes on nor cools down.
    RunnerStop,
}

/// A finished process: how it ended, and for a reviewer the verdict its review's event trail
/// records since it started, `None` when the runner did not or could not read it.
struct Processed {
    step: Step,
    trail_verdict: Option<TrailVerdict>,
}

impl Processed {
    fn of(step: Step) -> Self {
        Self {
            step,
            trail_verdict: None,
        }
    }
}

/// Works the pending reviews `console` lists with the author token, telling the agents the
/// console's `limits`, until `runner_stop` is requested. The state and ledger come back changed
/// even when the pass fails, so a pause or cooldown it set is kept.
pub async fn work_pending_reviews(
    settings: &RunnerSettings,
    console: &ConsoleClient,
    limits: AgentLimits,
    runner_stop: &RunnerStop,
    state: RunnerState,
    ledger: ProcessLedger,
) -> (Result<PassSummary, PassError>, RunnerState, ProcessLedger) {
    let pass = Pass {
        settings,
        console,
        limits,
        runner_stop,
        shared: Mutex::new(Shared {
            state,
            ledger,
            summary: PassSummary::default(),
            ledger_error: None,
        }),
    };
    let ran = pass.run().await;
    let Shared {
        state,
        ledger,
        summary,
        ledger_error,
    } = pass
        .shared
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner);
    let result = match (ran, ledger_error) {
        (Err(error), _) => Err(error),
        (Ok(()), Some(error)) => Err(PassError::Ledger(error)),
        (Ok(()), None) => Ok(summary),
    };
    (result, state, ledger)
}

#[derive(Debug, thiserror::Error)]
pub enum PassError {
    #[error("cannot list the pending reviews")]
    Console(#[source] ConsoleError),
    #[error(transparent)]
    Ledger(super::ledger::LedgerError),
}

impl Pass<'_> {
    /// Works the queue with a rolling pool: whenever a slot is free and the run's budget of
    /// `max_reviews_per_run` reviews is not spent, it reads the queue again and starts the next
    /// workable reviews, so a slot that frees never waits for the slowest process and the run
    /// picks up reviews that became workable meanwhile, such as one a human handed back. Each
    /// review is worked at most once per run, and one that came to wait for a human is no longer
    /// started. A pause stops the refill; the running processes finish, and the runner's stop is
    /// handled by the processes themselves.
    async fn run(&self) -> Result<(), PassError> {
        if let Some(stop) = self.stop(OffsetDateTime::now_utc()) {
            self.with(|shared| shared.summary.stopped = Some(stop));
            return Ok(());
        }
        let budget = usize::try_from(self.settings.max_reviews_per_run.get()).unwrap_or(usize::MAX);
        let concurrency = usize::try_from(self.settings.max_processes.get()).unwrap_or(1);
        let mut running = FuturesUnordered::new();
        let mut started = BTreeSet::new();
        let mut schema_started = 0;
        let mut refilling = true;
        while refilling || !running.is_empty() {
            let free = concurrency
                .saturating_sub(running.len())
                .min(budget.saturating_sub(started.len()));
            if refilling && free > 0 && !self.runner_stop.is_requested() {
                let first_read = started.is_empty();
                let schema_owed = budget.div_ceil(2).saturating_sub(schema_started);
                match self
                    .next_reviews(&started, free, schema_owed, first_read)
                    .await
                {
                    Ok(reviews) if reviews.is_empty() => {}
                    Ok(reviews) => match self.stop(OffsetDateTime::now_utc()) {
                        Some(stop) => {
                            self.with(|shared| shared.summary.stopped = Some(stop));
                            refilling = false;
                        }
                        None => {
                            for review in reviews {
                                started.insert(review.review_id);
                                schema_started += usize::from(
                                    review.artifact_type == ReviewArtifact::ProductSchema,
                                );
                                running.push(self.work_review(review));
                            }
                        }
                    },
                    Err(error) if first_read => return Err(error),
                    Err(error) => {
                        warn!(
                            error = %error,
                            cause = ?std::error::Error::source(&error),
                            "Cannot read the review queue to refill the run; it ends when its running processes do"
                        );
                        refilling = false;
                    }
                }
            }
            refilling &= started.len() < budget && !self.runner_stop.is_requested();
            if running.next().await.is_none() {
                break;
            }
        }
        if self.runner_stop.is_requested() {
            self.with(|shared| shared.summary.stopped = Some(PassStop::RunnerStop));
        }
        Ok(())
    }

    /// Reads the queue and picks the next `free` workable reviews no process of this run worked
    /// yet, `schema_owed` of them product-schema reviews as far as the queue has them. The first read also tells what the run found in the queue.
    async fn next_reviews(
        &self,
        started: &BTreeSet<CrawlerReviewId>,
        free: usize,
        schema_owed: usize,
        first_read: bool,
    ) -> Result<Vec<QueuedReview>, PassError> {
        let queue = review_queue(self.console)
            .await
            .map_err(PassError::Console)?;
        let pending = queue.workable.len();
        let waiting_for_human = queue.waiting_for_human.len();
        let unworked: Vec<QueuedReview> = queue
            .workable
            .into_iter()
            .filter(|review| !started.contains(&review.review_id))
            .collect();
        let (cooldowns, last_worked) =
            self.with(|shared| (shared.state.cooldown_ends(), shared.ledger.last_worked()));
        let (selected, cooling_down) = select_reviews(
            unworked,
            &cooldowns,
            &last_worked,
            OffsetDateTime::now_utc(),
            free,
            schema_owed,
        );
        if first_read {
            self.with(|shared| {
                shared.summary.pending = pending;
                shared.summary.waiting_for_human = waiting_for_human;
                shared.summary.cooling_down = cooling_down;
            });
            // The completion record carries the pass's counts at INFO once it worked a review.
            debug!(
                event = PASS_STARTED_EVENT,
                pending,
                waiting_for_human,
                cooling_down,
                selected = selected.len(),
                max_reviews_per_run = self.settings.max_reviews_per_run.get(),
                max_processes = self.settings.max_processes.get(),
                "Review agents pass started"
            );
        }
        Ok(selected)
    }

    /// The author's pass, then the reviewer's when the author left a candidate to decide. A review
    /// that waits for a human after either pass, whichever process's needs-repair verdict brought
    /// it to the cap, gets no further process and no cooldown.
    async fn work_review(&self, review: QueuedReview) {
        let author_report = match self.run_process(AgentRole::Author, &review).await.step {
            Step::NotStarted | Step::UsageLimit | Step::RunnerStop => return,
            Step::NoDecision => None,
            Step::Reported(report) => Some(report),
        };
        if self.waits_for_human_after(AgentRole::Author, &review).await {
            self.with(|shared| shared.summary.handed_to_human += 1);
            return;
        }
        if let Some(AgentReport::Author(report)) = &author_report
            && report.action == AuthorAction::NeedsHuman
        {
            self.hand_over_to_human(&review, report).await;
            return;
        }
        if let Some(AgentReport::Author(report)) = &author_report
            && report.action == AuthorAction::NothingToStage
            && self.reviewer_judged_review(&review).await
        {
            self.hand_over_to_human(&review, report).await;
            return;
        }
        if !leaves_a_candidate_to_decide(author_report.as_ref()) {
            self.cool_down(&review);
            return;
        }
        if self.guard_refuses(&review).await {
            self.with(|shared| shared.summary.guard_blocked += 1);
            self.cool_down(&review);
            return;
        }
        if let Some(until) = self.reviewer_pause_until(OffsetDateTime::now_utc()) {
            self.hold_back_reviewer(&review, until);
            return;
        }
        let processed = self.run_process(AgentRole::Reviewer, &review).await;
        match processed.step {
            Step::NotStarted | Step::UsageLimit | Step::RunnerStop => {}
            Step::Reported(report) => {
                self.pause_reviewers_at_decision_cap(&report).await;
                self.finish_reviewer_pass(&review, processed.trail_verdict)
                    .await;
            }
            Step::NoDecision => {
                self.finish_reviewer_pass(&review, processed.trail_verdict)
                    .await;
            }
        }
    }

    /// Whether the approval guard refuses the URL-pattern candidate the author left, so a reviewer
    /// process could only be refused, as the `TOO_FEW_LABELS` loops of #2822 were: a needs-repair
    /// round spent on what the author can read with `check_approval` itself. A product-schema
    /// review, and a review whose checks cannot be read, gets its reviewer as before.
    async fn guard_refuses(&self, review: &QueuedReview) -> bool {
        if review.artifact_type != ReviewArtifact::UrlPattern {
            return false;
        }
        match approval_failures(self.console, review.review_id).await {
            Ok(Some(failed)) if !failed.is_empty() => {
                info!(
                    review_id = %review.review_id,
                    failed_checks = ?failed,
                    "The approval guard refuses the candidate the author left; no reviewer process starts and the review cools down"
                );
                true
            }
            Ok(_) => false,
            Err(error) => {
                warn!(
                    review_id = %review.review_id,
                    error = %error,
                    cause = ?std::error::Error::source(&error),
                    "Cannot read the approval guard's checks of the candidate; the reviewer runs"
                );
                false
            }
        }
    }

    /// Whether the reviewer already judged the review's current candidate and pages, so a
    /// reviewer process after an author that changed nothing would only repeat its verdict as one
    /// more round. A review the runner cannot read counts as not judged: it gets its reviewer, as
    /// before.
    async fn reviewer_judged_review(&self, review: &QueuedReview) -> bool {
        match reviewer_judged_review(self.console, review.review_id).await {
            Ok(judged) => judged,
            Err(error) => {
                warn!(
                    review_id = %review.review_id,
                    error = %error,
                    cause = ?std::error::Error::source(&error),
                    "Cannot read whether the reviewer judged the candidate the author left unchanged; the reviewer runs"
                );
                false
            }
        }
    }

    /// Hands the review to a human at once with the author's reasoning, which the console records
    /// as an event of the review's trail, instead of starting a reviewer process: one that would
    /// repeat its verdict on the unchanged candidate, or one on a review the author left undecided. A hand-over the console does not take cools the
    /// review down unless it waits for a human anyway.
    async fn hand_over_to_human(&self, review: &QueuedReview, report: &AuthorReport) {
        match hand_to_human(self.console, review.review_id, &handover_note(report)).await {
            Ok(()) => {
                info!(
                    event = "crawler.review_agent.handed_to_human",
                    review_id = %review.review_id,
                    listing_source_id = %review.listing_source_id,
                    listing_source_name = review.listing_source_name.as_deref(),
                    artifact_type = %review.artifact_type,
                    "The author could not decide the review or found nothing to change on a candidate the reviewer already judged; the review is handed to a human and no reviewer process starts"
                );
                self.with(|shared| shared.summary.handed_to_human += 1);
            }
            Err(error) => {
                warn!(
                    review_id = %review.review_id,
                    error = %error,
                    cause = ?std::error::Error::source(&error),
                    "Cannot hand the review to a human"
                );
                if self.waits_for_human_after(AgentRole::Author, review).await {
                    self.with(|shared| shared.summary.handed_to_human += 1);
                } else {
                    self.cool_down(review);
                }
            }
        }
    }

    /// Counts the reviewer's decision, or cools the review down when it made none and does not
    /// wait for a human.
    async fn finish_reviewer_pass(
        &self,
        review: &QueuedReview,
        trail_verdict: Option<TrailVerdict>,
    ) {
        if trail_verdict.is_some_and(TrailVerdict::is_decision) {
            self.with(|shared| shared.summary.decided += 1);
        } else if !self
            .waits_for_human_after(AgentRole::Reviewer, review)
            .await
        {
            self.cool_down(review);
        }
    }

    /// Until when the reviewers pause at `now`, if they do. A pause that ended is forgotten and its
    /// end logged here, the first time a pass looks after it.
    fn reviewer_pause_until(&self, now: OffsetDateTime) -> Option<OffsetDateTime> {
        self.with(|shared| {
            if let Some(pause) = shared.state.reviewer_pause_at(now) {
                return Some(pause.until);
            }
            let ended = shared.state.reviewer_paused.take()?;
            log_reviewer_pause_ended(ended);
            self.keep_state(&shared.state);
            None
        })
    }

    /// The review's author staged, but no reviewer starts: it cools down until the reviewers'
    /// pause ends, so the pass does not run its author again while the reviewers pause and the
    /// review is workable again as soon as they resume.
    fn hold_back_reviewer(&self, review: &QueuedReview, until: OffsetDateTime) {
        debug!(
            review_id = %review.review_id,
            reviewers_paused_until = %timestamp(until),
            "No reviewer process starts: the reviewers pause after the agents' decision cap"
        );
        self.with(|shared| shared.summary.reviewers_held_back += 1);
        self.cool_down_until(review, until);
    }

    /// Pauses the reviewers when `report` shows the console's cap on the agents' decisions, until
    /// the time the console computes for its refusal. A trail the runner cannot read in full leaves
    /// that time unknown, and the pause then lasts the whole window the cap counts in.
    async fn pause_reviewers_at_decision_cap(&self, report: &AgentReport) {
        let AgentReport::Reviewer(report) = report else {
            return;
        };
        if !report_shows_decision_cap(report) {
            return;
        }
        let now = OffsetDateTime::now_utc();
        let until = match review_events_since(self.console, now - DECISION_WINDOW - CLOCK_MARGIN)
            .await
        {
            Ok(trail) if !trail.truncated => {
                match decision_cap_resumes_at(&trail.events, &self.limits, now) {
                    Some(until) => until,
                    None => {
                        warn!(
                            review_id = %report.review_id,
                            "A reviewer reported the agents' decision cap, but the event trail shows room under it; the reviewers do not pause"
                        );
                        return;
                    }
                }
            }
            Ok(_) => {
                warn!(
                    "The event trail of the last hour is cut short; the reviewers pause for the whole hour"
                );
                now + DECISION_WINDOW
            }
            Err(error) => {
                warn!(
                    error = %error,
                    cause = ?std::error::Error::source(&error),
                    "Cannot read the event trail for the agents' decision cap; the reviewers pause for the whole hour"
                );
                now + DECISION_WINDOW
            }
        };
        self.pause_reviewers(until, now);
    }

    /// Records the reviewer pause and logs its start, unless a pause that lasts as long is in force.
    fn pause_reviewers(&self, until: OffsetDateTime, now: OffsetDateTime) {
        self.with(|shared| {
            let in_force = shared.state.reviewer_pause_at(now);
            if in_force.is_some_and(|pause| pause.until >= until) {
                return;
            }
            shared.state.reviewer_paused = Some(ReviewerPause { until });
            self.keep_state(&shared.state);
            if in_force.is_none() {
                info!(
                    event = "crawler.review_agent.reviewers_paused",
                    resumes_at = %timestamp(until),
                    "The agents reached the console's decision cap; no reviewer process starts until it resumes, authors keep running"
                );
            }
        });
    }

    /// Whether `review` waits for a human after `role`'s pass, such as after a needs-repair
    /// verdict that reached the cap, also of a process that ended without a decision: the console
    /// would refuse everything a further process did on it with `HUMAN_REQUIRED`, and a cooldown
    /// would keep the agents from it after a human hands it back. A review the runner cannot read
    /// does not wait: it gets its reviewer, whose verdicts the console refuses should it wait, or
    /// its cooldown.
    async fn waits_for_human_after(&self, role: AgentRole, review: &QueuedReview) -> bool {
        match waits_for_human(self.console, review.review_id).await {
            Ok(true) => {
                info!(
                    event = "crawler.review_agent.waiting_for_human",
                    review_id = %review.review_id,
                    listing_source_id = %review.listing_source_id,
                    listing_source_name = review.listing_source_name.as_deref(),
                    artifact_type = %review.artifact_type,
                    role = role.as_str(),
                    "The review waits for a human after the agent's pass; no further process starts and it does not cool down"
                );
                true
            }
            Ok(false) => false,
            Err(error) => {
                warn!(
                    review_id = %review.review_id,
                    role = role.as_str(),
                    error = %error,
                    cause = ?std::error::Error::source(&error),
                    "Cannot read whether the review waits for a human after the agent's pass; it goes on as if it did not"
                );
                false
            }
        }
    }

    /// What the event trail records of the reviewer agent's verdict on `review` since `since`,
    /// the last one when it gave several. The trail, not the report, says what the console took:
    /// a reviewer may report a refused approval although its needs-repair verdict after it was
    /// accepted, report `NOT_SENT` for a verdict the trail shows, or end before it reports. A
    /// trail the runner cannot read gives `None`, which counts as no decision, so the review
    /// cools down.
    async fn reviewer_verdict(
        &self,
        review: &QueuedReview,
        since: OffsetDateTime,
    ) -> Option<TrailVerdict> {
        const DECISIONS: [(ReviewEventAction, TrailVerdict); 3] = [
            (ReviewEventAction::Approved, TrailVerdict::Approved),
            (ReviewEventAction::Rejected, TrailVerdict::Rejected),
            (
                ReviewEventAction::RepairRequested,
                TrailVerdict::RepairRequested,
            ),
        ];
        match review_events_since(self.console, since).await {
            Ok(trail) => Some(
                trail
                    .events
                    .iter()
                    .rev()
                    .filter(|event| {
                        event.review_id == review.review_id
                            && event.actor == ReviewActor::AgentReviewer.as_str()
                    })
                    .find_map(|event| {
                        DECISIONS
                            .iter()
                            .find(|(action, _)| event.action == action.as_str())
                            .map(|(_, verdict)| *verdict)
                    })
                    .unwrap_or(TrailVerdict::NoVerdict),
            ),
            Err(error) => {
                warn!(
                    review_id = %review.review_id,
                    error = %error,
                    cause = ?std::error::Error::source(&error),
                    "Cannot read the event trail for the reviewer's decision; the review cools down"
                );
                None
            }
        }
    }

    /// Runs one agent process of `role` on `review` and records it. After a reviewer process that
    /// ended without being stopped, it reads the event trail for the verdict the console took and
    /// records that with the process, next to the report's own claim.
    async fn run_process(&self, role: AgentRole, review: &QueuedReview) -> Processed {
        if let Some(stop) = self.stop(OffsetDateTime::now_utc()) {
            self.with(|shared| shared.summary.stopped = Some(stop));
            return Processed::of(Step::NotStarted);
        }
        let started = OffsetDateTime::now_utc();
        let contract = role_contract(role);
        let process = self.settings.role(role);
        if role == AgentRole::Author {
            self.with(|shared| shared.summary.worked += 1);
        }
        self.with(|shared| shared.summary.processes += 1);
        debug!(
            review_id = %review.review_id,
            artifact_type = %review.artifact_type,
            role = role.as_str(),
            prompt_version = contract.version,
            model = %process.model,
            effort = effort_flag(process.effort),
            max_turns = process.max_turns.get(),
            "Starting a review agent process"
        );
        let mut record = ProcessRecord::started(review, role, contract.version);
        let step = match agent_command(self.settings, &self.limits, role, review) {
            Ok(command) => {
                let output = run_agent(
                    &command,
                    self.settings.process_timeout,
                    self.runner_stop.agent_drain_ended(),
                )
                .await;
                self.read_output(&mut record, role, review, output)
            }
            Err(error) => record.failed(error.to_string()),
        };
        let trail_verdict = match (role, &step) {
            (AgentRole::Reviewer, Step::Reported(_) | Step::NoDecision) => {
                self.reviewer_verdict(review, started).await
            }
            _ => None,
        };
        record.trail_verdict = trail_verdict;
        record.finished = OffsetDateTime::now_utc();
        self.finish(review, record);
        Processed {
            step,
            trail_verdict,
        }
    }

    /// What a finished, stopped or failed process left.
    fn read_output(
        &self,
        record: &mut ProcessRecord,
        role: AgentRole,
        review: &QueuedReview,
        output: Result<ChildOutput, ChildError>,
    ) -> Step {
        let output = match output {
            Ok(output) => output,
            Err(error) => {
                warn!(
                    review_id = %review.review_id,
                    role = role.as_str(),
                    error = %error,
                    cause = ?std::error::Error::source(&error),
                    "The review agent process could not run"
                );
                return record.failed(error.to_string());
            }
        };
        let envelope = ResultEnvelope::parse(&output.stdout);
        record.context = ContextUse::parse(&output.stdout);
        self.pace(&UsageWindows::parse(&output.stdout));
        if let Some(stop) = output.stopped {
            if let Some(envelope) = &envelope {
                record.take_accounting(envelope);
            }
            self.charge_unreported_cost(record);
            return match stop {
                ChildStop::Timeout => record.failed(format!(
                    "stopped after its timeout of {} minutes",
                    self.settings.process_timeout.as_secs() / 60
                )),
                ChildStop::RunnerStop => {
                    record.failed("stopped when the runner's stop deadline passed".to_owned());
                    Step::RunnerStop
                }
            };
        }
        match envelope {
            Some(envelope) => self.read_envelope(record, role, review, &envelope),
            None => {
                warn!(
                    review_id = %review.review_id,
                    role = role.as_str(),
                    exit_code = ?output.exit_code,
                    stderr_tail = %output.stderr_tail,
                    "The review agent process printed no result envelope"
                );
                if output.exit_code.is_none() {
                    self.charge_unreported_cost(record);
                }
                record.failed(format!(
                    "no result envelope, exit code {:?}",
                    output.exit_code
                ))
            }
        }
    }

    /// Charges a process that was stopped at its timeout or by the runner's stop, or killed by a
    /// signal, and reported no cost the highest cost a process reported in the ledger's window, so
    /// the rolling sums count it. The CLI reports a cost only in its result envelope; the charge is
    /// an estimate, which a process that ran longer than any that reported may exceed, and zero
    /// while none reported.
    fn charge_unreported_cost(&self, record: &mut ProcessRecord) {
        if record.total_cost_usd > 0.0 {
            return;
        }
        record.total_cost_usd = self.with(|shared| shared.ledger.highest_reported_cost());
        record.cost_charged = true;
    }

    fn read_envelope(
        &self,
        record: &mut ProcessRecord,
        role: AgentRole,
        review: &QueuedReview,
        envelope: &ResultEnvelope,
    ) -> Step {
        record.take_accounting(envelope);
        let now = OffsetDateTime::now_utc();
        match envelope.outcome(now) {
            EnvelopeOutcome::Reported(output) => {
                let report = match role {
                    AgentRole::Author => AgentReport::author(output, review.review_id),
                    AgentRole::Reviewer => AgentReport::reviewer(output, review.review_id),
                };
                match report {
                    Ok(report) => {
                        record.end = ProcessEnd::Reported;
                        record.report = Some(report.clone());
                        Step::Reported(report)
                    }
                    Err(error) => record.failed(format!("unusable report: {error}")),
                }
            }
            EnvelopeOutcome::LimitReached(limit) => {
                record.end = ProcessEnd::LimitReached;
                record.limit = Some(limit);
                Step::NoDecision
            }
            EnvelopeOutcome::UsageLimit(limit) => {
                record.end = ProcessEnd::UsageLimit;
                record.usage_limit = Some(limit.kind);
                self.pause(limit, now);
                Step::UsageLimit
            }
            EnvelopeOutcome::Failed => record.failed("ended without a report".to_owned()),
        }
    }

    /// Records a finished process in the ledger and logs its outcome, usage and cost, never its
    /// report, prompt, page content or envelope.
    fn finish(&self, review: &QueuedReview, record: ProcessRecord) {
        info!(
            event = PROCESS_FINISHED_EVENT,
            review_id = %record.review_id,
            listing_source_id = %review.listing_source_id,
            listing_source_name = review.listing_source_name.as_deref(),
            artifact_type = %review.artifact_type,
            role = name(Some(record.role)),
            prompt_version = %record.prompt_version,
            end = name(Some(record.end)),
            limit = name(record.limit),
            usage_limit = name(record.usage_limit),
            decision = record.report.as_ref().map(decision_name),
            failure = non_empty(record.failure.as_deref()),
            subtype = non_empty(record.subtype.as_deref()),
            terminal_reason = non_empty(record.terminal_reason.as_deref()),
            api_error_status = record.api_error_status,
            num_turns = record.num_turns,
            duration_ms = record.duration_ms,
            models = %record.models.join(","),
            input_tokens = record.usage.input_tokens,
            output_tokens = record.usage.output_tokens,
            cache_read_input_tokens = record.usage.cache_read_input_tokens,
            cache_creation_input_tokens = record.usage.cache_creation_input_tokens,
            total_cost_usd = record.total_cost_usd,
            cost_charged = record.cost_charged,
            opening_tokens = record.context.opening_tokens,
            opening_cache_read_tokens = record.context.opening_cache_read_tokens,
            opening_cache_write_tokens = record.context.opening_cache_write_tokens,
            tool_answer_chars = record
                .context
                .tool_answers
                .values()
                .map(|answers| answers.chars)
                .sum::<u64>(),
            "Review agent process finished"
        );
        self.with(|shared| {
            if let Err(error) = shared.ledger.append(record)
                && shared.ledger_error.is_none()
            {
                shared.ledger_error = Some(error);
            }
        });
    }

    fn cool_down(&self, review: &QueuedReview) {
        self.cool_down_until(review, OffsetDateTime::now_utc() + self.settings.cooldown);
    }

    /// Lets `review` wait until `until` before its next pass.
    fn cool_down_until(&self, review: &QueuedReview, until: OffsetDateTime) {
        debug!(
            review_id = %review.review_id,
            until = %timestamp(until),
            "The review stays pending and cools down before its next pass"
        );
        self.with(|shared| {
            shared
                .state
                .cooldowns
                .insert(review.review_id, Cooldown { until });
            self.keep_state(&shared.state);
        });
    }

    /// Writes `state` right away, so a cooldown or pause outlives a run that is stopped before
    /// its end; the run writes it again when it ends and fails then if it still cannot.
    fn keep_state(&self, state: &RunnerState) {
        if let Err(error) = save_state(&self.settings.state_dir, state) {
            warn!(
                error = %error,
                cause = ?std::error::Error::source(&error),
                "Cannot write the runner's state yet; the run writes it when it ends"
            );
        }
    }

    fn pause(&self, limit: UsageLimit, now: OffsetDateTime) {
        let until = limit.paused_until(now);
        log_pause(&limit, until);
        self.pause_until(&limit, until, false);
    }

    /// Pauses before the agents take more than their share of the login's usage windows, as a
    /// process's stream last reported them ([`super::usage_pacing`]); running processes finish.
    fn pace(&self, windows: &UsageWindows) {
        let now = OffsetDateTime::now_utc();
        let Some(limit) = self.settings.pacing.pause(windows, now) else {
            return;
        };
        let until = limit.paused_until(now);
        info!(
            limit = name(Some(limit.kind)),
            five_hour_utilization = windows.five_hour.map(|window| window.utilization),
            seven_day_utilization = windows.seven_day.map(|window| window.utilization),
            until = %timestamp(until),
            "The agents took their share of the login's usage window; they pause until it allows more"
        );
        self.pause_until(&limit, until, true);
    }

    fn pause_until(&self, limit: &UsageLimit, until: OffsetDateTime, paced: bool) {
        self.with(|shared| {
            let later = shared.state.paused.is_none_or(|pause| pause.until < until);
            if later {
                shared.state.paused = Some(Pause {
                    until,
                    limit: limit.kind,
                    reported_reset: limit.resets_at.is_some(),
                    paced,
                });
                self.keep_state(&shared.state);
            }
        });
    }

    /// Why no further process may start at `now`, if any.
    fn stop(&self, now: OffsetDateTime) -> Option<PassStop> {
        if self.runner_stop.is_requested() {
            return Some(PassStop::RunnerStop);
        }
        self.with(|shared| {
            shared
                .state
                .pause_at(now)
                .map(|pause| PassStop::Paused(pause.until))
        })
    }

    fn with<T>(&self, change: impl FnOnce(&mut Shared) -> T) -> T {
        change(&mut self.shared.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// Whether the author's pass left a candidate for the reviewer to decide: the author reported
/// that it staged one or found nothing to stage, not that it skipped the review.
fn leaves_a_candidate_to_decide(author_report: Option<&AgentReport>) -> bool {
    author_report.is_some_and(|report| {
        !matches!(report, AgentReport::Author(report) if report.action == AuthorAction::Skipped)
    })
}

/// The note of the hand-over to a human: why no reviewer runs, then the author's own reasoning.
fn handover_note(report: &AuthorReport) -> String {
    let why = match report.action {
        AuthorAction::NeedsHuman => {
            "The author agent reported NEEDS_HUMAN: it cannot decide this review from what it can read, so no reviewer round runs."
        }
        _ => {
            "The author agent reported NOTHING_TO_STAGE on a candidate the reviewer agent had already judged, so no further reviewer round runs."
        }
    };
    format!("{why} Author's reasoning: {}", report.summary)
}

impl ProcessRecord {
    fn started(review: &QueuedReview, role: AgentRole, prompt_version: &str) -> Self {
        Self {
            finished: OffsetDateTime::now_utc(),
            review_id: review.review_id,
            role: role.into(),
            artifact_type: Some(review.artifact_type.as_str().to_owned()),
            prompt_version: prompt_version.to_owned(),
            end: ProcessEnd::Failed,
            limit: None,
            usage_limit: None,
            failure: None,
            subtype: None,
            terminal_reason: None,
            api_error_status: None,
            num_turns: 0,
            duration_ms: 0,
            models: Vec::new(),
            usage: Default::default(),
            total_cost_usd: 0.0,
            cost_charged: false,
            context: Default::default(),
            report: None,
            trail_verdict: None,
        }
    }

    /// Takes over what the envelope reports about the process's run: why it ended, its turns,
    /// duration, models, tokens and cost.
    fn take_accounting(&mut self, envelope: &ResultEnvelope) {
        self.subtype.clone_from(&envelope.subtype);
        self.terminal_reason.clone_from(&envelope.terminal_reason);
        self.api_error_status = envelope.api_error_status;
        self.num_turns = envelope.num_turns;
        self.duration_ms = envelope.duration_ms;
        self.models.clone_from(&envelope.models);
        self.usage = envelope.usage;
        self.total_cost_usd = envelope.total_cost_usd;
    }

    fn failed(&mut self, failure: String) -> Step {
        self.end = ProcessEnd::Failed;
        self.failure = Some(failure);
        Step::NoDecision
    }
}

impl PassStop {
    /// The stop for a log line.
    pub fn describe(stop: Option<Self>) -> String {
        match stop {
            Some(Self::Paused(until)) => format!("paused until {}", timestamp(until)),
            Some(Self::RunnerStop) => "the runner stops".to_owned(),
            None => String::new(),
        }
    }
}

/// The one word of a report the process log keeps: the author's action or the reviewer's
/// decision, in the vocabulary of `crawler.review.decided`. The report itself is in the ledger and
/// the decision in the console's event trail.
fn decision_name(report: &AgentReport) -> &'static str {
    match report {
        AgentReport::Author(report) => match report.action {
            AuthorAction::Staged => "staged",
            AuthorAction::NothingToStage => "nothing_to_stage",
            AuthorAction::Skipped => "skipped",
            AuthorAction::NeedsHuman => "needs_human",
        },
        AgentReport::Reviewer(report) => match report.decision {
            ReviewerDecision::Approve => "approved",
            ReviewerDecision::Reject => "rejected",
            ReviewerDecision::NeedsRepair => "needs_repair",
            ReviewerDecision::Skip => "skipped",
        },
    }
}

fn non_empty(text: Option<&str>) -> Option<&str> {
    text.filter(|text| !text.is_empty())
}

/// Logs the pause a usage limit sets. A limit whose reset time the runner cannot read logs the
/// line of the CLI's message, so a changed wording shows in the journal.
fn log_pause(limit: &UsageLimit, until: OffsetDateTime) {
    if limit.resets_at.is_some() {
        warn!(
            limit = name(Some(limit.kind)),
            reported_reset = true,
            until = %timestamp(until),
            "The CLI's login ran into a usage limit; the agents pause until it resets"
        );
    } else {
        warn!(
            limit = name(Some(limit.kind)),
            reported_reset = false,
            until = %timestamp(until),
            limit_message = limit.message.as_deref().unwrap_or(""),
            "The CLI reported a usage limit without a reset time the runner can read; the agents pause for an hour"
        );
    }
}

/// The kind names of the usage limits, for the digest.
pub fn usage_limit_name(kind: UsageLimitKind) -> &'static str {
    match kind {
        UsageLimitKind::Session => "session limit",
        UsageLimitKind::Weekly => "weekly limit",
        UsageLimitKind::Model => "model limit",
        UsageLimitKind::Spend => "spend limit",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::logged_events::LoggedEvents;

    #[test]
    fn should_log_the_limit_message_at_warn_when_its_reset_time_cannot_be_read() {
        let (logged, _guard) = LoggedEvents::capture();
        let now = OffsetDateTime::now_utc();
        let unreadable = UsageLimit {
            kind: UsageLimitKind::Weekly,
            resets_at: None,
            message: Some("You've hit your weekly limit · resets in a while".to_owned()),
        };
        let readable = UsageLimit {
            resets_at: Some(now + time::Duration::days(1)),
            ..unreadable.clone()
        };

        log_pause(&unreadable, unreadable.paused_until(now));
        log_pause(&readable, readable.paused_until(now));

        let events = logged.all();
        assert_eq!(events.len(), 2, "{events:?}");
        assert!(
            events
                .iter()
                .all(|logged| logged.level == tracing::Level::WARN)
        );
        assert_eq!(
            events[0].field("limit_message"),
            Some("You've hit your weekly limit · resets in a while")
        );
        assert!(events[1].field("limit_message").is_none(), "{events:?}");
    }
}
