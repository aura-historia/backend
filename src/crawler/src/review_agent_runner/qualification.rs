//! The qualification pass: before it works the reviews, each run works the ListingSources that
//! wait for their qualification as a real antique e-shop, one qualifier process per source, at
//! most `max_reviews_per_run` of them and `max_processes` at once. New sources come first, since
//! they are not crawled until they are decided; the sources that existed before qualification
//! did follow, which is the one-off backfill. The console decides everything: the qualifier calls
//! `qualify_source`, and its report only tells the runner what happened. Each process is recorded
//! in the qualification ledger, which the digest reads (`docs/crawler/operations.md`, review agent
//! runner).

use super::envelope::{EnvelopeOutcome, ResultEnvelope, TokenUsage};
use super::invocation::qualifier_command;
use super::ledger::ProcessEnd;
use super::process::run_agent;
use super::settings::RunnerSettings;
use super::state::{Pause, RunnerState};
use super::stop::RunnerStop;
use super::usage_pacing::UsageWindows;
use crate::review_mcp::{ConsoleClient, ConsoleError};
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
    #[serde(default)]
    pub num_turns: u64,
    #[serde(default)]
    pub usage: TokenUsage,
    #[serde(default)]
    pub total_cost_usd: f64,
}

impl QualificationRecord {
    /// Whether the decision replaced a label a human had set.
    pub fn replaced_human_label(&self) -> bool {
        self.previous_label
            .as_ref()
            .is_some_and(|label| label.labelled_by == "HUMAN")
            && self
                .report
                .as_ref()
                .is_some_and(|report| report.label_effect == "LABELLED")
    }

    /// Whether the qualifier qualified a labelled source without a label, so the source keeps
    /// the label against the decision and waits for a human.
    pub fn disagrees_with_label(&self) -> bool {
        self.report
            .as_ref()
            .is_some_and(|report| report.label_effect == "KEPT_LABEL")
    }
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
    let budget = usize::try_from(settings.max_reviews_per_run.get()).unwrap_or(usize::MAX);
    let concurrency = usize::try_from(settings.max_processes.get()).unwrap_or(1);
    let ledger_path = settings.state_dir.join(QUALIFICATION_LEDGER_FILE);
    let sources: Vec<QueuedSource> = queue.sources.into_iter().take(budget).collect();
    for batch in sources.chunks(concurrency.max(1)) {
        if stop.is_requested() || state.pause_at(OffsetDateTime::now_utc()).is_some() {
            summary.stopped = true;
            break;
        }
        let records =
            futures::future::join_all(batch.iter().map(|source| qualify(settings, stop, source)))
                .await;
        for (record, pause) in records {
            summary.processes += 1;
            summary.decided += usize::from(record.report.is_some());
            if let Some(pause) = pause {
                let later = state
                    .paused
                    .is_none_or(|current| current.until < pause.until);
                if later {
                    state.paused = Some(pause);
                }
            }
            log_record(&record);
            append(&ledger_path, &record)?;
        }
    }
    Ok(summary)
}

/// Runs the qualifier on `source` and answers its record, with the pause its stream asks for.
async fn qualify(
    settings: &RunnerSettings,
    stop: &RunnerStop,
    source: &QueuedSource,
) -> (QualificationRecord, Option<Pause>) {
    let mut record = QualificationRecord {
        finished: OffsetDateTime::now_utc(),
        listing_source_id: source.listing_source_id,
        listing_source_name: source.listing_source_name.clone(),
        backfill: source.backfill,
        previous_label: source.source_label.clone(),
        end: ProcessEnd::Failed,
        failure: None,
        report: None,
        num_turns: 0,
        usage: TokenUsage::default(),
        total_cost_usd: 0.0,
    };
    let command = match qualifier_command(settings, source) {
        Ok(command) => command,
        Err(error) => {
            record.failure = Some(error.to_string());
            return (record, None);
        }
    };
    let output = run_agent(&command, settings.process_timeout, stop.agent_drain_ended()).await;
    record.finished = OffsetDateTime::now_utc();
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            record.failure = Some(error.to_string());
            return (record, None);
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
        return (record, pause);
    };
    record.num_turns = envelope.num_turns;
    record.usage = envelope.usage;
    record.total_cost_usd = envelope.total_cost_usd;
    if output.stopped.is_some() {
        record.failure = Some("stopped before it reported".to_owned());
        return (record, pause);
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
        }
        EnvelopeOutcome::Failed => record.failure = Some("ended without a report".to_owned()),
    }
    (record, pause)
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
        qualification = report.map(|report| report.qualification.as_str()),
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

/// The digest's qualification section for `records`: counts per decision and label, the sources
/// that wait for a human, the human labels the backfill replaced, the decisions that disagree with
/// a label the source keeps, and the OTHER decisions grouped by reason.
pub fn digest_section(records: &[QualificationRecord]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "## Qualifications\n");
    if records.is_empty() {
        let _ = writeln!(out, "No qualifier process ran.\n");
        return out;
    }
    let reports: Vec<(&QualificationRecord, &QualifierReport)> = records
        .iter()
        .filter_map(|record| record.report.as_ref().map(|report| (record, report)))
        .collect();
    let mut by_outcome: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for (_, report) in &reports {
        *by_outcome
            .entry((report.qualification.as_str(), report.label.as_str()))
            .or_default() += 1;
    }
    let cost: f64 = records.iter().map(|record| record.total_cost_usd).sum();
    let backfill = records.iter().filter(|record| record.backfill).count();
    let _ = writeln!(
        out,
        "{} processes ({} of the backfill), {} reported, ${cost:.2}.\n",
        records.len(),
        backfill,
        reports.len()
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
    let row = |record: &QualificationRecord, report: &QualifierReport| {
        format!(
            "{} ({}): {}",
            record.listing_source_name, record.listing_source_id, report.summary
        )
    };
    list(
        "Waiting for a human",
        reports
            .iter()
            .filter(|(_, report)| report.qualification == "NEEDS_HUMAN")
            .map(|(record, report)| row(record, report))
            .collect(),
    );
    list(
        "Labels kept against the decision (a human clears or keeps them)",
        reports
            .iter()
            .filter(|(record, _)| record.disagrees_with_label())
            .map(|(record, report)| {
                format!(
                    "{} kept {} by {}",
                    row(record, report),
                    record
                        .previous_label
                        .as_ref()
                        .map_or("a label", |label| label.label.as_str()),
                    record
                        .previous_label
                        .as_ref()
                        .map_or("?", |label| label.labelled_by.as_str())
                )
            })
            .collect(),
    );
    list(
        "Human labels the backfill replaced",
        reports
            .iter()
            .filter(|(record, _)| record.replaced_human_label())
            .map(|(record, report)| {
                format!(
                    "{} ({}): {} → {}",
                    record.listing_source_name,
                    record.listing_source_id,
                    record
                        .previous_label
                        .as_ref()
                        .map_or("?", |label| label.label.as_str()),
                    report.label
                )
            })
            .collect(),
    );
    list(
        "Still on the retired APPROACH_NOT_APPLICABLE",
        reports
            .iter()
            .filter(|(record, report)| {
                record
                    .previous_label
                    .as_ref()
                    .is_some_and(|label| label.label == "APPROACH_NOT_APPLICABLE")
                    && report.label_effect != "LABELLED"
            })
            .map(|(record, report)| row(record, report))
            .collect(),
    );
    let mut other_reasons: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, report) in reports.iter().filter(|(_, report)| report.label == "OTHER") {
        let reason = if report.other_reason.is_empty() {
            "unnamed"
        } else {
            report.other_reason.as_str()
        };
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
    let failed: Vec<String> = records
        .iter()
        .filter(|record| record.report.is_none())
        .map(|record| {
            format!(
                "{} ({}): {}",
                record.listing_source_name,
                record.listing_source_id,
                record.failure.as_deref().unwrap_or("no report")
            )
        })
        .collect();
    list("Processes without a report", failed);
    out
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
            "7 processes (7 of the backfill), 7 reported, $1.75.",
            "- QUALIFIED · NONE: 3",
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
}
