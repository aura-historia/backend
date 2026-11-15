//! The qualification pass: before it works the reviews, each run works the ListingSources that wait
//! for their qualification as a real art and antiques e-shop, one qualifier process per source, at
//! most `max_reviews_per_run` of them and `max_processes` at once. New sources come first, since
//! they are not crawled until they are decided; the sources that existed before qualification did
//! follow, which is the one-off backfill. The console decides everything: the qualifier calls
//! `qualify_source`, and its report only tells the runner what happened. Each process is recorded
//! in the qualification ledger, which the digest reads (`docs/crawler/operations.md`, review agent
//! runner).

use super::envelope::{EnvelopeOutcome, ResultEnvelope, TokenUsage};
use super::invocation::qualifier_command;
use super::ledger::ProcessEnd;
use super::process::ChildStop;
use super::process::run_agent;
use super::settings::RunnerSettings;
use super::state::{Pause, QualificationFailures, RunnerState};
use super::stop::RunnerStop;
use super::usage_pacing::UsageWindows;
use crate::review_mcp::{ConsoleClient, ConsoleError};
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use listing_source_core::ListingSourceId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{self, BufRead, Write as _};
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use tracing::{info, warn};

/// The console's queue of the sources that wait for a qualification.
const QUALIFICATION_QUEUE_ROUTE: &str = "/api/source-qualifications";

/// The file in the runner's state directory that records every qualifier process.
pub const QUALIFICATION_LEDGER_FILE: &str = "qualification-processes.jsonl";

/// A source of the console's qualification queue, as the runner reads it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct QueuedSource {
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: String,
    pub crawl_root_url: String,
    pub backfill: bool,
    pub source_label: Option<QueuedSourceLabel>,
    #[serde(default)]
    pub pending_review_artifacts: Vec<String>,
    /// The note of the agents' earlier `NEEDS_HUMAN` this try follows, if any.
    #[serde(default)]
    pub earlier_note: Option<String>,
}

/// The label a queued source carries, by its persisted names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedSourceLabel {
    pub label: String,
    pub note: Option<String>,
    pub labelled_by: String,
}

#[derive(Debug, Deserialize)]
struct QualificationQueue {
    total: usize,
    sources: Vec<QueuedSource>,
    /// How many more decisions the agents' hourly cap admits now; every qualifier the pass starts
    /// may take one.
    agent_decisions_left: usize,
}

/// The qualifier's report, as its report schema has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QualifierReport {
    pub listing_source_id: String,
    pub qualification: String,
    pub label: String,
    pub label_effect: String,
    #[serde(default)]
    pub other_reason: String,
    pub summary: String,
}

/// The most characters of a report's text the ledger keeps.
const MAX_REPORT_TEXT_CHARS: usize = 600;

impl QualifierReport {
    /// The report in `output` for `listing_source_id`, its texts bounded; `None` for a report
    /// that does not decode or names another source.
    fn of(output: serde_json::Value, listing_source_id: ListingSourceId) -> Option<Self> {
        let report: Self = serde_json::from_value(output).ok()?;
        (report.listing_source_id.trim() == listing_source_id.to_string()).then(|| Self {
            summary: report.summary.chars().take(MAX_REPORT_TEXT_CHARS).collect(),
            other_reason: report
                .other_reason
                .trim()
                .to_lowercase()
                .chars()
                .take(60)
                .collect(),
            ..report
        })
    }
}

/// What the console stores for a source after its qualifier process, which the digest reads
/// instead of the agent's own report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredOutcome {
    /// The qualification the process decided; `None` when it decided none.
    pub qualification: Option<String>,
    /// The label the source carries now, with who set it.
    pub label: Option<String>,
    pub labelled_by: Option<String>,
}

/// `GET /api/listing-sources/{id}` as far as the runner reads it.
#[derive(Debug, Deserialize)]
struct SourceDetail {
    qualification: Option<SourceQualificationAnswer>,
    source_label: Option<SourceLabelAnswer>,
}

#[derive(Debug, Deserialize)]
struct SourceQualificationAnswer {
    qualification: String,
    #[serde(with = "time::serde::rfc3339")]
    qualified_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct SourceLabelAnswer {
    label: String,
    labelled_by: String,
}

/// One finished qualifier process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualificationRecord {
    #[serde(with = "time::serde::rfc3339")]
    pub finished: OffsetDateTime,
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: String,
    pub backfill: bool,
    /// The label the source carried when the process started, which its decision may replace.
    pub previous_label: Option<QueuedSourceLabel>,
    pub end: ProcessEnd,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<QualifierReport>,
    /// What the console stored after the process; `None` when the runner could not read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stored: Option<StoredOutcome>,
    /// Whether the runner handed the source to a human after this process, the last of
    /// [`MAX_QUALIFIER_FAILURES`] without a decision.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub handed_over: bool,
    #[serde(default)]
    pub num_turns: u64,
    #[serde(default)]
    pub usage: TokenUsage,
    #[serde(default)]
    pub total_cost_usd: f64,
}

impl QualificationRecord {
    /// Why the process ended without a decision, in the runner's words, or how it ended.
    pub fn failure_text(&self) -> String {
        self.failure
            .clone()
            .unwrap_or_else(|| format!("{:?}", self.end))
    }

    /// The qualification the process decided as the console stored it, else as the agent
    /// reported it.
    pub fn decided(&self) -> Option<&str> {
        match &self.stored {
            Some(stored) => stored.qualification.as_deref(),
            None => self
                .report
                .as_ref()
                .map(|report| report.qualification.as_str())
                .filter(|qualification| *qualification != "NOT_DECIDED"),
        }
    }

    /// The label the source carries after the process, as the console stored it, else as the
    /// agent reported it.
    pub fn label_after(&self) -> Option<&str> {
        match (&self.stored, &self.report) {
            (Some(stored), _) => stored.label.as_deref(),
            (None, Some(report)) if report.label_effect == "KEPT_LABEL" => self
                .previous_label
                .as_ref()
                .map(|label| label.label.as_str()),
            (None, report) => report
                .as_ref()
                .map(|report| report.label.as_str())
                .filter(|label| *label != "NONE"),
        }
    }

    /// Whether the decision replaced a label a human had set.
    pub fn replaced_human_label(&self) -> bool {
        let Some(previous) = self
            .previous_label
            .as_ref()
            .filter(|label| label.labelled_by == "HUMAN")
        else {
            return false;
        };
        match &self.stored {
            Some(stored) => {
                stored
                    .labelled_by
                    .as_deref()
                    .is_some_and(|by| by != "HUMAN")
                    || (stored.label.is_some()
                        && stored.label.as_deref() != Some(previous.label.as_str()))
            }
            None => self
                .report
                .as_ref()
                .is_some_and(|report| report.label_effect == "LABELLED"),
        }
    }

    /// Whether the qualifier qualified a labelled source without a label, so the source keeps
    /// the label against the decision and waits for a human.
    pub fn disagrees_with_label(&self) -> bool {
        match &self.stored {
            Some(stored) => {
                stored.qualification.as_deref() == Some("QUALIFIED")
                    && stored.label.is_some()
                    && stored.label.as_ref()
                        == self.previous_label.as_ref().map(|label| &label.label)
            }
            None => self
                .report
                .as_ref()
                .is_some_and(|report| report.label_effect == "KEPT_LABEL"),
        }
    }
}

/// How many qualifier processes of a source may end without a decision in a row before the
/// runner hands the source to a human.
pub const MAX_QUALIFIER_FAILURES: u32 = 3;

/// How a finished process counts for its source's run of failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Attempt {
    /// The console stored a decision.
    Decided,
    /// It ended without a decision for a reason of the source, such as a timeout, the turn
    /// limit or an unusable report.
    Failed,
    /// It ended for a reason of the runner, such as a usage limit or the runner's stop, which
    /// says nothing about the source.
    NotCounted,
}

/// What one qualification pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QualificationSummary {
    /// Sources waiting when the pass read the queue.
    pub waiting: usize,
    /// Qualifier processes started.
    pub processes: usize,
    /// Processes that reported a decision.
    pub decided: usize,
    /// Whether the pass stopped early for a usage limit or the runner's stop.
    pub stopped: bool,
    /// Whether the agents' hourly decision cap held sources back: the pass started no more
    /// qualifiers than decisions were left, so none is refused after reading the shop.
    pub decision_capped: bool,
}

/// The sources a pass starts qualifiers for, in queue order: at most `per_run`, and no more than
/// the agents' hourly cap has `decisions_left`, since every qualifier may take a decision and one
/// the cap refuses would end without a decision after reading the shop, counting against its
/// source. Answers whether the cap held sources back, which then wait for the next run.
fn sources_to_qualify(
    workable: Vec<QueuedSource>,
    per_run: usize,
    decisions_left: usize,
) -> (Vec<QueuedSource>, bool) {
    let capped = decisions_left < per_run && workable.len() > decisions_left;
    let to_qualify = workable
        .into_iter()
        .take(per_run.min(decisions_left))
        .collect();
    (to_qualify, capped)
}

#[derive(Debug, thiserror::Error)]
pub enum QualificationError {
    #[error("cannot read the qualification queue")]
    Console(#[source] ConsoleError),
    #[error("cannot write the qualification ledger {}", path.display())]
    Ledger {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Works the qualification queue until its budget is spent, `state` holds a usage pause or
/// `stop` is requested; a usage limit a process runs into pauses `state` as the review pass does.
pub async fn qualify_waiting_sources(
    settings: &RunnerSettings,
    console: &ConsoleClient,
    stop: &RunnerStop,
    state: &mut RunnerState,
) -> Result<QualificationSummary, QualificationError> {
    let mut summary = QualificationSummary::default();
    if state.pause_at(OffsetDateTime::now_utc()).is_some() || stop.is_requested() {
        summary.stopped = true;
        return Ok(summary);
    }
    let queue: QualificationQueue = console
        .get(QUALIFICATION_QUEUE_ROUTE)
        .await
        .and_then(|answer| answer.decode())
        .map_err(QualificationError::Console)?;
    summary.waiting = queue.total;
    let per_run = usize::try_from(settings.max_reviews_per_run.get()).unwrap_or(usize::MAX);
    let concurrency = usize::try_from(settings.max_processes.get()).unwrap_or(1);
    let ledger_path = settings.state_dir.join(QUALIFICATION_LEDGER_FILE);
    let now = OffsetDateTime::now_utc();
    let cooling_down = |source: &QueuedSource| {
        state
            .qualification_failures
            .get(&source.listing_source_id)
            .is_some_and(|failures| failures.until > now)
    };
    let workable: Vec<QueuedSource> = queue
        .sources
        .into_iter()
        .filter(|source| !cooling_down(source))
        .collect();
    let (to_qualify, decision_capped) =
        sources_to_qualify(workable, per_run, queue.agent_decisions_left);
    summary.decision_capped = decision_capped;
    let mut waiting = to_qualify.into_iter();
    // A rolling pool: a slot that frees starts the next source at once, so one slow process
    // never holds back the others.
    let mut running = FuturesUnordered::new();
    loop {
        while running.len() < concurrency.max(1) {
            if stop.is_requested() || state.pause_at(OffsetDateTime::now_utc()).is_some() {
                summary.stopped = true;
                break;
            }
            let Some(source) = waiting.next() else {
                break;
            };
            running.push(qualify(settings, console, stop, source));
        }
        let Some((mut record, pause, attempt)) = running.next().await else {
            break;
        };
        count_attempt(settings, console, state, &mut record, attempt).await;
        summary.processes += 1;
        summary.decided += usize::from(attempt == Attempt::Decided);
        if let Some(pause) = pause
            && state
                .paused
                .is_none_or(|current| current.until < pause.until)
        {
            state.paused = Some(pause);
        }
        log_record(&record);
        append(&ledger_path, &record)?;
    }
    Ok(summary)
}

/// Runs the qualifier on `source` and answers its record, with the pause its stream asks for and
/// how it counts for the source's run of failures.
async fn qualify(
    settings: &RunnerSettings,
    console: &ConsoleClient,
    stop: &RunnerStop,
    source: QueuedSource,
) -> (QualificationRecord, Option<Pause>, Attempt) {
    let started = OffsetDateTime::now_utc();
    let (mut record, pause, ended_for_runner) = run_qualifier(settings, stop, &source).await;
    record.stored = stored_outcome(console, source.listing_source_id, started).await;
    let attempt = match (&record.stored, ended_for_runner) {
        (Some(stored), _) if stored.qualification.is_some() => Attempt::Decided,
        (None, _) if record.decided().is_some() => Attempt::Decided,
        (_, true) => Attempt::NotCounted,
        _ => Attempt::Failed,
    };
    (record, pause, attempt)
}

/// What the console stores for the source after a process that started at `started`: the
/// qualification only when that process decided it. `None` when the console cannot be read.
async fn stored_outcome(
    console: &ConsoleClient,
    listing_source_id: ListingSourceId,
    started: OffsetDateTime,
) -> Option<StoredOutcome> {
    let detail: SourceDetail = match console
        .get(&format!("/api/listing-sources/{listing_source_id}"))
        .await
        .and_then(|answer| answer.decode())
    {
        Ok(detail) => detail,
        Err(error) => {
            warn!(
                listing_source_id = %listing_source_id,
                error = %error,
                "Cannot read the console's qualification of the source; the digest uses the agent's report"
            );
            return None;
        }
    };
    Some(StoredOutcome {
        qualification: detail
            .qualification
            .filter(|qualification| qualification.qualified_at >= started)
            .map(|qualification| qualification.qualification),
        labelled_by: detail
            .source_label
            .as_ref()
            .map(|label| label.labelled_by.clone()),
        label: detail.source_label.map(|label| label.label),
    })
}

/// Counts `record`'s process for its source: a decision ends the source's run of failures, a
/// failure lets the source cool down, and the [`MAX_QUALIFIER_FAILURES`]th failure in a row hands
/// the source to a human with the agents' `NEEDS_HUMAN`.
async fn count_attempt(
    settings: &RunnerSettings,
    console: &ConsoleClient,
    state: &mut RunnerState,
    record: &mut QualificationRecord,
    attempt: Attempt,
) {
    let listing_source_id = record.listing_source_id;
    match attempt {
        Attempt::NotCounted => {}
        Attempt::Decided => {
            state.qualification_failures.remove(&listing_source_id);
        }
        Attempt::Failed => {
            let in_a_row = state
                .qualification_failures
                .get(&listing_source_id)
                .map_or(0, |failures| failures.in_a_row)
                + 1;
            if in_a_row < MAX_QUALIFIER_FAILURES {
                state.qualification_failures.insert(
                    listing_source_id,
                    QualificationFailures {
                        in_a_row,
                        until: OffsetDateTime::now_utc() + settings.cooldown,
                    },
                );
                return;
            }
            state.qualification_failures.remove(&listing_source_id);
            record.handed_over = hand_over(console, record, in_a_row).await;
        }
    }
}

/// Decides `NEEDS_HUMAN` for the source of `record`, whose last `in_a_row` processes ended
/// without a decision, and answers whether the console took it; a refusal is logged, and the
/// source waits in the queue as before.
async fn hand_over(console: &ConsoleClient, record: &QualificationRecord, in_a_row: u32) -> bool {
    let last = record.failure_text();
    let note: String = format!(
        "{in_a_row} qualifier processes in a row ended without a decision; the last: {last}"
    )
    .chars()
    .take(500)
    .collect();
    let path = format!("/api/source-qualifications/{}", record.listing_source_id);
    let body = serde_json::json!({ "qualification": "NEEDS_HUMAN", "note": note });
    match console.post(&path, &body).await {
        Ok(_) => {
            info!(
            event = "crawler.review_agent.qualification_handed_to_human",
            listing_source_id = %record.listing_source_id,
            listing_source_name = %record.listing_source_name,
            failures = in_a_row,
            "Qualifier processes kept ending without a decision; the source waits for a human"
            );
            true
        }
        Err(error) => {
            warn!(
                listing_source_id = %record.listing_source_id,
                error = %error,
                "Cannot hand the source to a human after its qualifier processes failed"
            );
            false
        }
    }
}

/// Runs one qualifier process on `source`: its record, the pause its stream asks for, and whether
/// it ended for a reason of the runner rather than of the source.
async fn run_qualifier(
    settings: &RunnerSettings,
    stop: &RunnerStop,
    source: &QueuedSource,
) -> (QualificationRecord, Option<Pause>, bool) {
    let mut record = QualificationRecord {
        finished: OffsetDateTime::now_utc(),
        listing_source_id: source.listing_source_id,
        listing_source_name: source.listing_source_name.clone(),
        backfill: source.backfill,
        previous_label: source.source_label.clone(),
        end: ProcessEnd::Failed,
        failure: None,
        report: None,
        stored: None,
        handed_over: false,
        num_turns: 0,
        usage: TokenUsage::default(),
        total_cost_usd: 0.0,
    };
    let command = match qualifier_command(settings, source) {
        Ok(command) => command,
        Err(error) => {
            record.failure = Some(error.to_string());
            return (record, None, true);
        }
    };
    let output = run_agent(&command, settings.process_timeout, stop.agent_drain_ended()).await;
    record.finished = OffsetDateTime::now_utc();
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            record.failure = Some(error.to_string());
            return (record, None, true);
        }
    };
    let now = OffsetDateTime::now_utc();
    let mut pause = settings
        .pacing
        .pause(&UsageWindows::parse(&output.stdout), now)
        .map(|limit| Pause {
            until: limit.paused_until(now),
            limit: limit.kind,
            reported_reset: limit.resets_at.is_some(),
            paced: true,
        });
    let Some(envelope) = ResultEnvelope::parse(&output.stdout) else {
        record.failure = Some(format!(
            "no result envelope, exit code {:?}",
            output.exit_code
        ));
        return (record, pause, false);
    };
    record.num_turns = envelope.num_turns;
    record.usage = envelope.usage;
    record.total_cost_usd = envelope.total_cost_usd;
    if let Some(stopped) = output.stopped {
        record.failure = Some("stopped before it reported".to_owned());
        return (record, pause, stopped == ChildStop::RunnerStop);
    }
    match envelope.outcome(now) {
        EnvelopeOutcome::Reported(report) => {
            match QualifierReport::of(report, source.listing_source_id) {
                Some(report) => {
                    record.end = ProcessEnd::Reported;
                    record.report = Some(report);
                }
                None => record.failure = Some("unusable report".to_owned()),
            }
        }
        EnvelopeOutcome::LimitReached(_) => record.end = ProcessEnd::LimitReached,
        EnvelopeOutcome::UsageLimit(limit) => {
            record.end = ProcessEnd::UsageLimit;
            pause = Some(Pause {
                until: limit.paused_until(now),
                limit: limit.kind,
                reported_reset: limit.resets_at.is_some(),
                paced: false,
            });
            return (record, pause, true);
        }
        EnvelopeOutcome::Failed => record.failure = Some("ended without a report".to_owned()),
    }
    (record, pause, false)
}

/// Logs a finished process with its decision, never its summary or the source's pages.
fn log_record(record: &QualificationRecord) {
    let report = record.report.as_ref();
    info!(
        event = "crawler.review_agent.qualification_finished",
        listing_source_id = %record.listing_source_id,
        listing_source_name = %record.listing_source_name,
        backfill = record.backfill,
        end = ?record.end,
        qualification = record.decided(),
        label = report.map(|report| report.label.as_str()),
        label_effect = report.map(|report| report.label_effect.as_str()),
        failure = record.failure.as_deref(),
        num_turns = record.num_turns,
        total_cost_usd = record.total_cost_usd,
        "Qualifier process finished"
    );
}

fn append(path: &Path, record: &QualificationRecord) -> Result<(), QualificationError> {
    let failed = |source| QualificationError::Ledger {
        path: path.to_owned(),
        source,
    };
    let line = serde_json::to_string(record).map_err(|error| failed(io::Error::other(error)))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(failed)?;
    writeln!(file, "{line}").map_err(failed)
}

/// The qualification ledger's records that finished at or after `since`; a line that is no
/// record is skipped with a warning, so one bad line costs the digest that line only.
pub fn records_since(state_dir: &Path, since: OffsetDateTime) -> Vec<QualificationRecord> {
    let path = state_dir.join(QUALIFICATION_LEDGER_FILE);
    let Ok(file) = std::fs::File::open(&path) else {
        return Vec::new();
    };
    io::BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| match serde_json::from_str::<QualificationRecord>(&line) {
            Ok(record) => Some(record),
            Err(error) => {
                warn!(path = %path.display(), error = %error, "Skipping an unreadable qualification record");
                None
            }
        })
        .filter(|record| record.finished >= since)
        .collect()
}

/// The digest's qualification section for `records`, from what the console stored after each
/// process (the agent's report only where the console could not be read): counts per decision and
/// label, the sources that wait for a human, the human labels the backfill replaced, the decisions
/// that disagree with a label the source keeps, the sources still on the retired label, and the
/// OTHER decisions grouped by the reason the qualifier names.
pub fn digest_section(records: &[QualificationRecord]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "## Qualifications\n");
    if records.is_empty() {
        let _ = writeln!(out, "No qualifier process ran.\n");
        return out;
    }
    let decided: Vec<(&QualificationRecord, &str)> = records
        .iter()
        .filter_map(|record| {
            record
                .decided()
                .map(|qualification| (record, qualification))
        })
        .collect();
    let mut by_outcome: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for (record, qualification) in &decided {
        *by_outcome
            .entry((qualification, record.label_after().unwrap_or("NONE")))
            .or_default() += 1;
    }
    let cost: f64 = records.iter().map(|record| record.total_cost_usd).sum();
    let backfill = records.iter().filter(|record| record.backfill).count();
    let _ = writeln!(
        out,
        "{} processes ({} of the backfill), {} decided, ${cost:.2}.\n",
        records.len(),
        backfill,
        decided.len()
    );
    for ((qualification, label), count) in &by_outcome {
        let _ = writeln!(out, "- {qualification} · {label}: {count}");
    }
    out.push('\n');
    let mut list = |title: &str, rows: Vec<String>| {
        let _ = writeln!(out, "### {title}\n");
        if rows.is_empty() {
            let _ = writeln!(out, "None.\n");
        } else {
            for row in rows {
                let _ = writeln!(out, "- {row}");
            }
            out.push('\n');
        }
    };
    let row = |record: &QualificationRecord| {
        let summary = record
            .report
            .as_ref()
            .map_or("no report", |report| report.summary.as_str());
        format!(
            "{} ({}): {summary}",
            record.listing_source_name, record.listing_source_id
        )
    };
    list(
        "Waiting for a human",
        records
            .iter()
            .filter(|record| record.decided() == Some("NEEDS_HUMAN") || record.handed_over)
            .map(|record| {
                if record.handed_over {
                    format!(
                        "{} ({}): handed over after {MAX_QUALIFIER_FAILURES} processes without a decision; the last: {}",
                        record.listing_source_name,
                        record.listing_source_id,
                        record.failure_text()
                    )
                } else {
                    row(record)
                }
            })
            .collect(),
    );
    list(
        "Labels kept against the decision (a human clears or keeps them)",
        records
            .iter()
            .filter(|record| record.disagrees_with_label())
            .map(|record| {
                let (label, by) = previous(record);
                format!("{} kept {label} by {by}", row(record))
            })
            .collect(),
    );
    list(
        "Human labels the backfill replaced",
        records
            .iter()
            .filter(|record| record.replaced_human_label())
            .map(|record| {
                format!(
                    "{} ({}): {} → {}",
                    record.listing_source_name,
                    record.listing_source_id,
                    previous(record).0,
                    record.label_after().unwrap_or("NONE")
                )
            })
            .collect(),
    );
    list(
        "Still on the retired APPROACH_NOT_APPLICABLE",
        records
            .iter()
            .filter(|record| record.label_after() == Some("APPROACH_NOT_APPLICABLE"))
            .map(row)
            .collect(),
    );
    let mut other_reasons: BTreeMap<&str, usize> = BTreeMap::new();
    for (record, _) in decided
        .iter()
        .filter(|(record, _)| record.label_after() == Some("OTHER"))
    {
        let reason = record
            .report
            .as_ref()
            .map(|report| report.other_reason.as_str())
            .filter(|reason| !reason.is_empty())
            .unwrap_or("unnamed");
        *other_reasons.entry(reason).or_default() += 1;
    }
    let mut reasons: Vec<(&str, usize)> = other_reasons.into_iter().collect();
    reasons.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(right.0)));
    list(
        "OTHER by reason (candidate labels for a follow-up)",
        reasons
            .into_iter()
            .map(|(reason, count)| format!("{reason}: {count}"))
            .collect(),
    );
    list(
        "Processes without a decision",
        records
            .iter()
            .filter(|record| record.decided().is_none())
            .map(|record| {
                format!(
                    "{} ({}): {}",
                    record.listing_source_name,
                    record.listing_source_id,
                    record.failure_text()
                )
            })
            .collect(),
    );
    out
}

/// The label `record`'s source carried before the process and who set it, `?` for each when the
/// record names none.
fn previous(record: &QualificationRecord) -> (&str, &str) {
    record.previous_label.as_ref().map_or(("?", "?"), |label| {
        (label.label.as_str(), label.labelled_by.as_str())
    })
}

/// The cost of the qualifier processes of `records` that finished within `window` before `now`,
/// which the runner's rolling sums add to the review agents'.
pub fn cost_within(
    records: &[QualificationRecord],
    window: time::Duration,
    now: OffsetDateTime,
) -> f64 {
    records
        .iter()
        .filter(|record| record.finished > now - window)
        .map(|record| record.total_cost_usd)
        .sum()
}

/// Drops the records that finished before `before` from the qualification ledger in `state_dir`,
/// as the process ledger keeps only its retention.
pub fn prune(state_dir: &Path, before: OffsetDateTime) -> Result<(), QualificationError> {
    let path = state_dir.join(QUALIFICATION_LEDGER_FILE);
    let failed = |source| QualificationError::Ledger {
        path: path.clone(),
        source,
    };
    if !path.exists() {
        return Ok(());
    }
    let kept: Vec<String> = records_since(state_dir, before)
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<_, _>>()
        .map_err(|error| failed(io::Error::other(error)))?;
    let temporary = path.with_extension("jsonl.tmp");
    let mut text = kept.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    std::fs::write(&temporary, text).map_err(failed)?;
    std::fs::rename(&temporary, &path).map_err(failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(
        previous: Option<(&str, &str)>,
        qualification: &str,
        label: &str,
        effect: &str,
        other_reason: &str,
    ) -> QualificationRecord {
        QualificationRecord {
            finished: time::macros::datetime!(2026-11-14 10:00:00 UTC),
            listing_source_id: ListingSourceId::new(),
            listing_source_name: "Shop".to_owned(),
            backfill: true,
            previous_label: previous.map(|(label, by)| QueuedSourceLabel {
                label: label.to_owned(),
                note: None,
                labelled_by: by.to_owned(),
            }),
            end: ProcessEnd::Reported,
            failure: None,
            stored: None,
            handed_over: false,
            report: Some(QualifierReport {
                listing_source_id: String::new(),
                qualification: qualification.to_owned(),
                label: label.to_owned(),
                label_effect: effect.to_owned(),
                other_reason: other_reason.to_owned(),
                summary: "read /".to_owned(),
            }),
            num_turns: 4,
            usage: TokenUsage::default(),
            total_cost_usd: 0.25,
        }
    }

    fn waiting_source(index: usize) -> QueuedSource {
        QueuedSource {
            listing_source_id: ListingSourceId::new(),
            listing_source_name: format!("Shop {index}"),
            crawl_root_url: format!("https://shop{index}.example/"),
            backfill: true,
            source_label: None,
            pending_review_artifacts: Vec::new(),
            earlier_note: None,
        }
    }

    /// The console's queue answer carries the decisions the agents have left this hour.
    #[test]
    fn should_read_the_decisions_left_with_the_queue() {
        let queue: QualificationQueue = serde_json::from_value(json!({
            "total": 0,
            "sources": [],
            "agent_decisions_left": 7,
        }))
        .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(queue.agent_decisions_left, 7);
    }

    /// A pass starts no more qualifiers than the hourly cap has decisions left, in queue order, and
    /// says when the cap held sources back; with room it starts its per-run budget.
    #[test]
    fn should_start_no_more_qualifiers_than_the_cap_has_decisions_left() {
        let workable: Vec<QueuedSource> = (0..5).map(waiting_source).collect();
        let first = |count: usize| workable[..count].to_vec();

        assert_eq!(
            sources_to_qualify(workable.clone(), 60, 2),
            (first(2), true)
        );
        assert_eq!(
            sources_to_qualify(workable.clone(), 60, 0),
            (Vec::new(), true)
        );
        assert_eq!(
            sources_to_qualify(workable.clone(), 3, 300),
            (first(3), false)
        );
        assert_eq!(
            sources_to_qualify(workable.clone(), 60, 5),
            (first(5), false)
        );
    }

    #[test]
    fn should_read_only_a_report_for_the_source_it_was_given() {
        let listing_source_id = ListingSourceId::new();
        let output = |id: String| {
            json!({
                "listing_source_id": id,
                "qualification": "DISQUALIFIED",
                "label": "OTHER",
                "label_effect": "LABELLED",
                "other_reason": " Auction Platform ",
                "summary": "x".repeat(MAX_REPORT_TEXT_CHARS + 10),
            })
        };

        let report = QualifierReport::of(output(listing_source_id.to_string()), listing_source_id)
            .unwrap_or_else(|| panic!("a report for the source"));
        assert_eq!(report.other_reason, "auction platform");
        assert_eq!(report.summary.chars().count(), MAX_REPORT_TEXT_CHARS);
        assert_eq!(
            None,
            QualifierReport::of(
                output(ListingSourceId::new().to_string()),
                listing_source_id
            )
        );
        assert_eq!(None, QualifierReport::of(json!({}), listing_source_id));
    }

    #[test]
    fn should_list_disagreements_replaced_human_labels_and_other_reasons_in_the_digest() {
        let records = vec![
            record(None, "QUALIFIED", "NONE", "NONE", ""),
            record(
                Some(("OTHER", "HUMAN")),
                "QUALIFIED",
                "NONE",
                "KEPT_LABEL",
                "",
            ),
            record(
                Some(("NOT_AN_ESHOP", "HUMAN")),
                "DISQUALIFIED",
                "NOT_AN_ANTIQUE_SHOP",
                "LABELLED",
                "",
            ),
            record(
                Some(("APPROACH_NOT_APPLICABLE", "AGENT_AUTHOR")),
                "QUALIFIED",
                "NONE",
                "KEPT_LABEL",
                "",
            ),
            record(
                None,
                "DISQUALIFIED",
                "OTHER",
                "LABELLED",
                "auction platform",
            ),
            record(None, "QUALIFIED", "OTHER", "LABELLED", "auction platform"),
            record(None, "NEEDS_HUMAN", "NONE", "NONE", ""),
        ];

        let section = digest_section(&records);

        for expected in [
            "7 processes (7 of the backfill), 7 decided, $1.75.",
            "- QUALIFIED · NONE: 1",
            "- QUALIFIED · OTHER: 2",
            "- QUALIFIED · APPROACH_NOT_APPLICABLE: 1",
            "- NEEDS_HUMAN · NONE: 1",
            "kept OTHER by HUMAN",
            "kept APPROACH_NOT_APPLICABLE by AGENT_AUTHOR",
            "NOT_AN_ESHOP → NOT_AN_ANTIQUE_SHOP",
            "auction platform: 2",
        ] {
            assert!(section.contains(expected), "{expected}\n{section}");
        }
        let retired = section
            .split("### Still on the retired APPROACH_NOT_APPLICABLE")
            .nth(1)
            .and_then(|rest| rest.split("###").next())
            .unwrap_or_default();
        assert_eq!(retired.matches("- Shop").count(), 1, "{retired}");
    }

    /// The console's stored outcome decides over the agent's report: a report that claims a
    /// decision the console did not store counts as none, and a label the source keeps against a
    /// qualification shows as kept, whatever the report says.
    #[test]
    fn should_read_the_digest_from_what_the_console_stored() {
        let stored = |qualification: Option<&str>, label: Option<(&str, &str)>| StoredOutcome {
            qualification: qualification.map(str::to_owned),
            label: label.map(|(label, _)| label.to_owned()),
            labelled_by: label.map(|(_, by)| by.to_owned()),
        };
        let mut claimed = record(None, "QUALIFIED", "NONE", "NONE", "");
        claimed.stored = Some(stored(None, None));
        let mut kept = record(Some(("OTHER", "HUMAN")), "QUALIFIED", "NONE", "NONE", "");
        kept.stored = Some(stored(Some("QUALIFIED"), Some(("OTHER", "HUMAN"))));
        let mut replaced = record(Some(("OTHER", "HUMAN")), "DISQUALIFIED", "NONE", "NONE", "");
        replaced.stored = Some(stored(
            Some("DISQUALIFIED"),
            Some(("SHOP_CLOSED", "AGENT_AUTHOR")),
        ));

        assert_eq!(claimed.decided(), None);
        assert!(kept.disagrees_with_label() && !kept.replaced_human_label());
        assert!(replaced.replaced_human_label() && !replaced.disagrees_with_label());
        let section = digest_section(&[claimed, kept, replaced]);
        for expected in [
            "3 processes (3 of the backfill), 2 decided",
            "- DISQUALIFIED · SHOP_CLOSED: 1",
            "- QUALIFIED · OTHER: 1",
            "OTHER → SHOP_CLOSED",
        ] {
            assert!(section.contains(expected), "{expected}\n{section}");
        }
    }

    fn runner_settings() -> RunnerSettings {
        crate::review_agent_runner::settings::RunnerSettings::from_args_and_env(
            Vec::new(),
            |_| None,
            &crate::review_agent_runner::settings::RunnerLocation {
                working_dir: PathBuf::from("/var/lib/crawler"),
                executable: PathBuf::from("/opt/crawler/bin/review-agent-runner"),
            },
        )
        .unwrap_or_else(|error| panic!("{error}"))
    }

    /// A process without a decision lets its source cool down and counts it; a decision forgets
    /// the count; a process the runner ended counts for nothing. No request reaches the console
    /// below the cap.
    #[tokio::test]
    async fn should_cool_a_failing_source_down_and_forget_it_once_decided() {
        let settings = runner_settings();
        let directory =
            std::env::temp_dir().join(format!("qualification-tokens-{}", ListingSourceId::new()));
        std::fs::create_dir_all(&directory).unwrap_or_else(|error| panic!("{error}"));
        let token_file = directory.join("review-agent-tokens.json");
        std::fs::write(
            &token_file,
            r#"{"author":"authortoken","reviewer":"reviewertoken"}"#,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        // Nothing listens there: the hand-over at the cap is refused and logged.
        let console = ConsoleClient::new(
            &url::Url::parse("http://127.0.0.1:9").unwrap_or_else(|error| panic!("{error}")),
            crate::review_mcp::RoleTokenFile::new(token_file, crate::review_mcp::AgentRole::Author),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let mut state = RunnerState::default();
        let mut failed = record(None, "NOT_DECIDED", "NONE", "NONE", "");
        let source = failed.listing_source_id;

        for in_a_row in 1..MAX_QUALIFIER_FAILURES {
            count_attempt(
                &settings,
                &console,
                &mut state,
                &mut failed,
                Attempt::Failed,
            )
            .await;
            let failures = state.qualification_failures[&source];
            assert_eq!(failures.in_a_row, in_a_row);
            assert!(failures.until > OffsetDateTime::now_utc());
        }
        count_attempt(
            &settings,
            &console,
            &mut state,
            &mut failed,
            Attempt::NotCounted,
        )
        .await;
        assert_eq!(
            state.qualification_failures[&source].in_a_row,
            MAX_QUALIFIER_FAILURES - 1
        );
        count_attempt(
            &settings,
            &console,
            &mut state,
            &mut failed,
            Attempt::Decided,
        )
        .await;
        assert!(state.qualification_failures.is_empty());

        for _ in 0..MAX_QUALIFIER_FAILURES {
            count_attempt(
                &settings,
                &console,
                &mut state,
                &mut failed,
                Attempt::Failed,
            )
            .await;
        }
        assert!(
            state.qualification_failures.is_empty(),
            "the last failure hands the source over, which the console decides; a refusal is logged"
        );
        assert!(
            !failed.handed_over,
            "an unreachable console took no hand-over"
        );
        std::fs::remove_dir_all(&directory).unwrap_or_default();
    }

    #[test]
    fn should_sum_the_cost_within_the_window_and_prune_older_records() {
        let directory =
            std::env::temp_dir().join(format!("qualification-ledger-{}", ListingSourceId::new()));
        std::fs::create_dir_all(&directory).unwrap_or_else(|error| panic!("{error}"));
        let now = time::macros::datetime!(2026-11-14 12:00:00 UTC);
        let mut old = record(None, "QUALIFIED", "NONE", "NONE", "");
        old.finished = now - time::Duration::days(10);
        let mut recent = record(None, "QUALIFIED", "NONE", "NONE", "");
        recent.finished = now - time::Duration::hours(1);
        let path = directory.join(QUALIFICATION_LEDGER_FILE);
        for record in [&old, &recent] {
            append(&path, record).unwrap_or_else(|error| panic!("{error}"));
        }

        assert!(
            (cost_within(
                &[old.clone(), recent.clone()],
                time::Duration::hours(5),
                now
            ) - 0.25)
                .abs()
                < 1e-9
        );
        prune(&directory, now - time::Duration::days(7)).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            records_since(&directory, OffsetDateTime::UNIX_EPOCH),
            vec![recent]
        );
        std::fs::remove_dir_all(&directory).unwrap_or_default();
    }
}
