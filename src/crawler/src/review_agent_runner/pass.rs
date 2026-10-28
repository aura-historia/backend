//! One pass of the agents over the pending reviews: per review a fresh author process, then, when
//! the author staged or kept a candidate, a fresh reviewer process, with at most the configured
//! number of processes at once. Each process's outcome is recorded in the ledger and logged.
//! A process that ends without a decision cools its review down; a usage limit pauses the runner
//! until its reset, and a soft budget that is spent starts no further process. A review that waits
//! for a human is no agent work: the pass starts no process for it, so it neither fails nor cools
//! down.

use super::digest::timestamp;
use super::envelope::{EnvelopeOutcome, ResultEnvelope, UsageLimit, UsageLimitKind};
use super::invocation::{agent_command, role_contract};
use super::ledger::{FIVE_HOURS, ONE_WEEK, ProcessEnd, ProcessLedger, ProcessRecord};
use super::process::{ChildError, ChildOutput, run_agent};
use super::queue::{QueuedReview, review_queue, select_reviews};
use super::reports::{AgentReport, AuthorAction};
use super::settings::{RunnerSettings, effort_flag};
use super::state::{Cooldown, Pause, RunnerState, save_state};
use super::wire_name as name;
use crate::review_mcp::{AgentRole, ConsoleClient, ConsoleError};
use futures::StreamExt;
use std::sync::{Mutex, PoisonError};
use time::OffsetDateTime;
use tracing::{info, warn};

/// What a pass did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PassSummary {
    /// Pending reviews the agents could work.
    pub pending: usize,
    /// Pending reviews that wait for a human, which no process works.
    pub waiting_for_human: usize,
    /// Of those, reviews still cooling down.
    pub cooling_down: usize,
    /// Reviews this pass started to work.
    pub worked: usize,
    pub processes: usize,
    /// Reviews the server took a reviewer decision on.
    pub decided: usize,
    /// Why the pass started no or no further process, if it stopped early.
    pub stopped: Option<PassStop>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PassStop {
    /// A usage limit of the CLI's login pauses the runner until then.
    Paused(OffsetDateTime),
    /// A rolling cost sum reached its soft budget.
    BudgetSpent,
}

/// What a pass works with and changes: the runner's state and ledger.
struct Pass<'a> {
    settings: &'a RunnerSettings,
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
    /// The pass started no process: it is paused or its budget is spent.
    NotStarted,
    Reported(AgentReport),
    /// It ended without a decision; its review cools down.
    NoDecision,
    /// The CLI's login ran into a usage limit; the runner pauses.
    UsageLimit,
}

/// Works the pending reviews `console` lists with the author token. The state and ledger come
/// back changed even when the pass fails, so a pause or cooldown it set is kept.
pub async fn work_pending_reviews(
    settings: &RunnerSettings,
    console: &ConsoleClient,
    state: RunnerState,
    ledger: ProcessLedger,
) -> (Result<PassSummary, PassError>, RunnerState, ProcessLedger) {
    let pass = Pass {
        settings,
        shared: Mutex::new(Shared {
            state,
            ledger,
            summary: PassSummary::default(),
            ledger_error: None,
        }),
    };
    let ran = pass.run(console).await;
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
    async fn run(&self, console: &ConsoleClient) -> Result<(), PassError> {
        if let Some(stop) = self.stop(OffsetDateTime::now_utc()) {
            self.with(|shared| shared.summary.stopped = Some(stop));
            return Ok(());
        }
        let queue = review_queue(console).await.map_err(PassError::Console)?;
        let pending = queue.workable.len();
        let waiting_for_human = queue.waiting_for_human.len();
        let (cooldowns, last_worked) =
            self.with(|shared| (shared.state.cooldown_ends(), shared.ledger.last_worked()));
        let (selected, cooling_down) = select_reviews(
            queue.workable,
            &cooldowns,
            &last_worked,
            OffsetDateTime::now_utc(),
            usize::try_from(self.settings.max_reviews_per_run.get()).unwrap_or(usize::MAX),
        );
        self.with(|shared| {
            shared.summary.pending = pending;
            shared.summary.waiting_for_human = waiting_for_human;
            shared.summary.cooling_down = cooling_down;
        });
        info!(
            pending,
            waiting_for_human,
            cooling_down,
            selected = selected.len(),
            max_reviews_per_run = self.settings.max_reviews_per_run.get(),
            max_processes = self.settings.max_processes.get(),
            "Review agents pass started"
        );
        let concurrency = usize::try_from(self.settings.max_processes.get()).unwrap_or(1);
        futures::stream::iter(selected)
            .map(|review| self.work_review(review))
            .buffer_unordered(concurrency)
            .collect::<Vec<()>>()
            .await;
        Ok(())
    }

    /// The author's pass, then the reviewer's when the author left a candidate to decide.
    async fn work_review(&self, review: QueuedReview) {
        match self.run_process(AgentRole::Author, &review).await {
            Step::NotStarted | Step::UsageLimit => return,
            Step::NoDecision => {
                self.cool_down(&review);
                return;
            }
            Step::Reported(AgentReport::Author(report))
                if report.action == AuthorAction::Skipped =>
            {
                self.cool_down(&review);
                return;
            }
            Step::Reported(_) => {}
        }
        match self.run_process(AgentRole::Reviewer, &review).await {
            Step::NotStarted | Step::UsageLimit => {}
            Step::Reported(AgentReport::Reviewer(report)) if report.decided() => {
                self.with(|shared| shared.summary.decided += 1);
            }
            Step::Reported(_) | Step::NoDecision => self.cool_down(&review),
        }
    }

    async fn run_process(&self, role: AgentRole, review: &QueuedReview) -> Step {
        if let Some(stop) = self.stop(OffsetDateTime::now_utc()) {
            self.with(|shared| shared.summary.stopped = Some(stop));
            return Step::NotStarted;
        }
        let contract = role_contract(role);
        let process = self.settings.role(role);
        if role == AgentRole::Author {
            self.with(|shared| shared.summary.worked += 1);
        }
        self.with(|shared| shared.summary.processes += 1);
        info!(
            review_id = %review.review_id,
            artifact_type = %review.artifact_type,
            role = role.as_str(),
            prompt_version = contract.version,
            model = %process.model,
            effort = effort_flag(process.effort),
            max_turns = process.max_turns.get(),
            max_budget_usd = process.max_budget_usd,
            "Starting a review agent process"
        );
        let mut record = ProcessRecord::started(review, role, contract.version);
        let step = match agent_command(self.settings, role, review) {
            Ok(command) => {
                let output = run_agent(&command, self.settings.process_timeout).await;
                self.read_output(&mut record, role, review, output)
            }
            Err(error) => record.failed(error.to_string()),
        };
        record.finished = OffsetDateTime::now_utc();
        self.finish(record);
        step
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
        if output.timed_out {
            return record.failed(format!(
                "stopped after its timeout of {} minutes",
                self.settings.process_timeout.as_secs() / 60
            ));
        }
        match ResultEnvelope::parse(&output.stdout) {
            Some(envelope) => self.read_envelope(record, role, review, &envelope),
            None => {
                warn!(
                    review_id = %review.review_id,
                    role = role.as_str(),
                    exit_code = ?output.exit_code,
                    stderr_tail = %output.stderr_tail,
                    "The review agent process printed no result envelope"
                );
                record.failed(format!(
                    "no result envelope, exit code {:?}",
                    output.exit_code
                ))
            }
        }
    }

    fn read_envelope(
        &self,
        record: &mut ProcessRecord,
        role: AgentRole,
        review: &QueuedReview,
        envelope: &ResultEnvelope,
    ) -> Step {
        record.subtype.clone_from(&envelope.subtype);
        record.terminal_reason.clone_from(&envelope.terminal_reason);
        record.api_error_status = envelope.api_error_status;
        record.num_turns = envelope.num_turns;
        record.duration_ms = envelope.duration_ms;
        record.models.clone_from(&envelope.models);
        record.usage = envelope.usage;
        record.total_cost_usd = envelope.total_cost_usd;
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

    /// Records and logs a finished process: its report, usage and cost, never its prompt, page
    /// content or envelope.
    fn finish(&self, record: ProcessRecord) {
        let report = record
            .report
            .as_ref()
            .and_then(|report| serde_json::to_string(report).ok())
            .unwrap_or_default();
        info!(
            review_id = %record.review_id,
            role = name(Some(record.role)),
            prompt_version = %record.prompt_version,
            end = name(Some(record.end)),
            limit = name(record.limit),
            usage_limit = name(record.usage_limit),
            failure = record.failure.as_deref().unwrap_or(""),
            subtype = record.subtype.as_deref().unwrap_or(""),
            terminal_reason = record.terminal_reason.as_deref().unwrap_or(""),
            api_error_status = record.api_error_status,
            num_turns = record.num_turns,
            duration_ms = record.duration_ms,
            models = %record.models.join(","),
            input_tokens = record.usage.input_tokens,
            output_tokens = record.usage.output_tokens,
            cache_read_input_tokens = record.usage.cache_read_input_tokens,
            cache_creation_input_tokens = record.usage.cache_creation_input_tokens,
            total_cost_usd = record.total_cost_usd,
            report = %report,
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
        let until = OffsetDateTime::now_utc() + self.settings.cooldown;
        info!(
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
        warn!(
            limit = name(Some(limit.kind)),
            reported_reset = limit.resets_at.is_some(),
            until = %timestamp(until),
            "The CLI's login ran into a usage limit; the agents pause until it resets"
        );
        self.with(|shared| {
            let later = shared.state.paused.is_none_or(|pause| pause.until < until);
            if later {
                shared.state.paused = Some(Pause {
                    until,
                    limit: limit.kind,
                    reported_reset: limit.resets_at.is_some(),
                });
                self.keep_state(&shared.state);
            }
        });
    }

    /// Why no further process may start at `now`, if any.
    fn stop(&self, now: OffsetDateTime) -> Option<PassStop> {
        self.with(|shared| {
            if let Some(pause) = shared.state.pause_at(now) {
                return Some(PassStop::Paused(pause.until));
            }
            let five_hours = shared.ledger.cost_within(FIVE_HOURS, now);
            let week = shared.ledger.cost_within(ONE_WEEK, now);
            if five_hours >= self.settings.five_hour_budget_usd
                || week >= self.settings.weekly_budget_usd
            {
                warn!(
                    five_hour_cost_usd = five_hours,
                    five_hour_budget_usd = self.settings.five_hour_budget_usd,
                    weekly_cost_usd = week,
                    weekly_budget_usd = self.settings.weekly_budget_usd,
                    "A rolling cost sum reached its soft budget; no further agent process starts"
                );
                return Some(PassStop::BudgetSpent);
            }
            None
        })
    }

    fn with<T>(&self, change: impl FnOnce(&mut Shared) -> T) -> T {
        change(&mut self.shared.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl ProcessRecord {
    fn started(review: &QueuedReview, role: AgentRole, prompt_version: &str) -> Self {
        Self {
            finished: OffsetDateTime::now_utc(),
            review_id: review.review_id,
            role: role.into(),
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
            report: None,
        }
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
            Some(Self::BudgetSpent) => "soft budget spent".to_owned(),
            None => String::new(),
        }
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
