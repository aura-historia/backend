//! One pass of the agents over the pending reviews: per review a fresh author process, then, when
//! the author staged or kept a candidate, a fresh reviewer process, with at most the configured
//! number of processes at once. Each process's outcome is recorded in the ledger and logged.
//! A process that ends without a decision cools its review down, and a usage limit pauses the
//! runner until its reset. Whether a reviewer decided is what the review's event trail records,
//! not what its report says the console answered. A review that waits for a human is no agent
//! work: the pass starts no process for it, nor a reviewer process once the author's pass handed
//! it to a human, so it neither fails nor cools down. Once the runner stops, the pass starts no
//! process and stops the running ones, which it records with their cost; their reviews do not
//! cool down.

use super::digest::timestamp;
use super::envelope::{EnvelopeOutcome, ResultEnvelope, UsageLimit, UsageLimitKind};
use super::invocation::{agent_command, role_contract};
use super::ledger::{ProcessEnd, ProcessLedger, ProcessRecord};
use super::process::{ChildError, ChildOutput, ChildStop, run_agent};
use super::queue::{
    QueuedReview, review_events_since, review_queue, select_reviews, waits_for_human,
};
use super::reports::{AgentReport, AuthorAction};
use super::settings::{RunnerSettings, effort_flag};
use super::state::{Cooldown, Pause, RunnerState, save_state};
use super::stop::RunnerStop;
use super::wire_name as name;
use crate::review::agent_decisions::AgentLimits;
use crate::review::model::{ReviewActor, ReviewEventAction};
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
    async fn run(&self) -> Result<(), PassError> {
        if let Some(stop) = self.stop(OffsetDateTime::now_utc()) {
            self.with(|shared| shared.summary.stopped = Some(stop));
            return Ok(());
        }
        let queue = review_queue(self.console)
            .await
            .map_err(PassError::Console)?;
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
        if self.runner_stop.is_requested() {
            self.with(|shared| shared.summary.stopped = Some(PassStop::RunnerStop));
        }
        Ok(())
    }

    /// The author's pass, then the reviewer's when the author left a candidate to decide.
    async fn work_review(&self, review: QueuedReview) {
        let author_report = match self.run_process(AgentRole::Author, &review).await {
            Step::NotStarted | Step::UsageLimit | Step::RunnerStop => return,
            Step::NoDecision => {
                self.cool_down(&review);
                return;
            }
            Step::Reported(report) => report,
        };
        if self.handed_to_human(&review).await {
            self.with(|shared| shared.summary.handed_to_human += 1);
            return;
        }
        if let AgentReport::Author(report) = &author_report
            && report.action == AuthorAction::Skipped
        {
            self.cool_down(&review);
            return;
        }
        let reviewer_started = OffsetDateTime::now_utc();
        match self.run_process(AgentRole::Reviewer, &review).await {
            Step::NotStarted | Step::UsageLimit | Step::RunnerStop => {}
            Step::Reported(_) | Step::NoDecision => {
                if self.reviewer_decided(&review, reviewer_started).await {
                    self.with(|shared| shared.summary.decided += 1);
                } else {
                    self.cool_down(&review);
                }
            }
        }
    }

    /// Whether `review` waits for a human after the author's pass, such as after the author's own
    /// needs-repair verdict reached the cap: the console would refuse everything a reviewer
    /// process did on it with `HUMAN_REQUIRED`. A review the runner cannot read gets its
    /// reviewer; should it wait for a human, the console refuses that reviewer's verdicts.
    async fn handed_to_human(&self, review: &QueuedReview) -> bool {
        match waits_for_human(self.console, review.review_id).await {
            Ok(true) => {
                info!(
                    review_id = %review.review_id,
                    "The review waits for a human after the author's pass; no reviewer process starts and it does not cool down"
                );
                true
            }
            Ok(false) => false,
            Err(error) => {
                warn!(
                    review_id = %review.review_id,
                    error = %error,
                    cause = ?std::error::Error::source(&error),
                    "Cannot read whether the review waits for a human; its reviewer process starts"
                );
                false
            }
        }
    }

    /// Whether the event trail records a reviewer agent's decision on `review` since `since`.
    /// The trail, not the report, says what the console took: a reviewer may report a refused
    /// approval although its needs-repair verdict after it was accepted, or end before it reports.
    /// A trail the runner cannot read counts as no decision, so the review cools down.
    async fn reviewer_decided(&self, review: &QueuedReview, since: OffsetDateTime) -> bool {
        const DECISIONS: [ReviewEventAction; 3] = [
            ReviewEventAction::Approved,
            ReviewEventAction::Rejected,
            ReviewEventAction::RepairRequested,
        ];
        match review_events_since(self.console, since).await {
            Ok(trail) => trail.events.iter().any(|event| {
                event.review_id == review.review_id
                    && event.actor == ReviewActor::AgentReviewer.as_str()
                    && DECISIONS
                        .iter()
                        .any(|decision| event.action == decision.as_str())
            }),
            Err(error) => {
                warn!(
                    review_id = %review.review_id,
                    error = %error,
                    cause = ?std::error::Error::source(&error),
                    "Cannot read the event trail for the reviewer's decision; the review cools down"
                );
                false
            }
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
            "Starting a review agent process"
        );
        let mut record = ProcessRecord::started(review, role, contract.version);
        let step = match agent_command(self.settings, &self.limits, role, review) {
            Ok(command) => {
                let output = run_agent(
                    &command,
                    self.settings.process_timeout,
                    self.runner_stop.requested(),
                )
                .await;
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
        let envelope = ResultEnvelope::parse(&output.stdout);
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
                    record.failed("stopped when the runner stopped".to_owned());
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
            cost_charged = record.cost_charged,
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
        log_pause(&limit, until);
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
            cost_charged: false,
            report: None,
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
    use std::fmt::Write as _;
    use std::sync::Arc;
    use tracing::field::{Field, Visit};
    use tracing_subscriber::layer::{Context, SubscriberExt};

    /// The events logged while it is the default subscriber: each one's level and fields.
    #[derive(Clone, Default)]
    struct LoggedEvents(Arc<Mutex<Vec<(tracing::Level, String)>>>);

    struct Fields(String);

    impl Visit for Fields {
        fn record_str(&mut self, field: &Field, value: &str) {
            let _ = write!(self.0, "{}={value} ", field.name());
        }

        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            let _ = write!(self.0, "{}={value:?} ", field.name());
        }
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for LoggedEvents {
        fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
            let mut fields = Fields(String::new());
            event.record(&mut fields);
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((*event.metadata().level(), fields.0));
        }
    }

    #[test]
    fn should_log_the_limit_message_at_warn_when_its_reset_time_cannot_be_read() {
        let logged = LoggedEvents::default();
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

        tracing::subscriber::with_default(
            tracing_subscriber::registry().with(logged.clone()),
            || {
                tracing::callsite::rebuild_interest_cache();
                log_pause(&unreadable, unreadable.paused_until(now));
                log_pause(&readable, readable.paused_until(now));
            },
        );

        let events = logged.0.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(events.len(), 2, "{events:?}");
        assert!(
            events
                .iter()
                .all(|(level, _)| *level == tracing::Level::WARN)
        );
        assert!(
            events[0]
                .1
                .contains("limit_message=You've hit your weekly limit · resets in a while"),
            "{events:?}"
        );
        assert!(!events[1].1.contains("limit_message"), "{events:?}");
    }
}
