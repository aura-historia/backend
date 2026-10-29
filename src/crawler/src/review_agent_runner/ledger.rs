//! The runner's ledger of agent processes: one JSON line per finished process with its review,
//! role, outcome, report, tokens and cost, appended to `review-agent-processes.jsonl` in the state
//! directory; a cost the runner charged a process stopped before it reported one is marked as
//! such. It is what the rolling cost sums and the digest's reports read; prompts, envelopes and
//! page content never reach it. Records older than the longest window anything reads are pruned
//! at the start of a run.

use super::envelope::{ProcessLimit, TokenUsage, UsageLimitKind};
use super::reports::AgentReport;
use crate::CrawlerReviewId;
use crate::review_mcp::AgentRole;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use time::{Duration, OffsetDateTime};

pub const LEDGER_FILE: &str = "review-agent-processes.jsonl";
/// How long records are kept: the weekly cost window plus a day for the digest.
pub const LEDGER_RETENTION: Duration = Duration::days(8);
/// The rolling windows the runner sums `total_cost_usd` over.
pub const FIVE_HOURS: Duration = Duration::hours(5);
pub const ONE_WEEK: Duration = Duration::days(7);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessRole {
    Author,
    Reviewer,
}

impl From<AgentRole> for ProcessRole {
    fn from(role: AgentRole) -> Self {
        match role {
            AgentRole::Author => Self::Author,
            AgentRole::Reviewer => Self::Reviewer,
        }
    }
}

/// How an agent process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProcessEnd {
    /// It ended its turn with the role's report.
    Reported,
    /// It hit its turn, budget or context limit before it decided.
    LimitReached,
    /// The CLI's login ran into a plan limit.
    UsageLimit,
    /// It ended without a usable report for any other reason.
    Failed,
}

/// One finished agent process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessRecord {
    #[serde(with = "time::serde::rfc3339")]
    pub finished: OffsetDateTime,
    pub review_id: CrawlerReviewId,
    pub role: ProcessRole,
    /// The prompt and report schema version, such as `author.v2`.
    pub prompt_version: String,
    pub end: ProcessEnd,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<ProcessLimit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_limit: Option<UsageLimitKind>,
    /// Why a failed process failed, in the runner's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    #[serde(default)]
    pub subtype: Option<String>,
    #[serde(default)]
    pub terminal_reason: Option<String>,
    #[serde(default)]
    pub api_error_status: Option<u16>,
    #[serde(default)]
    pub num_turns: u64,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub usage: TokenUsage,
    #[serde(default)]
    pub total_cost_usd: f64,
    /// Whether `total_cost_usd` is the runner's charge for a process that was stopped or killed
    /// before it reported a cost, not what the CLI reported.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cost_charged: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<AgentReport>,
}

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("cannot read or write the runner's process ledger {}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("line {line} of the runner's process ledger {} is no process record", path.display())]
    Malformed {
        path: PathBuf,
        line: usize,
        #[source]
        source: serde_json::Error,
    },
}

/// The ledger and the records it holds.
#[derive(Debug)]
pub struct ProcessLedger {
    path: PathBuf,
    records: Vec<ProcessRecord>,
}

impl ProcessLedger {
    /// Reads the ledger in `state_dir`; a missing file is an empty ledger. A last line without its
    /// newline is a record an interrupted run did not finish writing: kept and given its newline
    /// when it is whole, cut off otherwise, so the next record starts a line of its own. Any other
    /// line that is no record fails the read, so cost sums never silently lose records.
    pub fn open(state_dir: &Path) -> Result<Self, LedgerError> {
        let path = state_dir.join(LEDGER_FILE);
        let io_error = |source| LedgerError::Io {
            path: path.clone(),
            source,
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(io_error(error)),
        };
        let complete = text.is_empty() || text.ends_with('\n');
        let lines: Vec<&str> = text.lines().collect();
        let mut records = Vec::with_capacity(lines.len());
        let mut unfinished_whole = false;
        for (index, line) in lines.iter().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let last_unfinished = !complete && index + 1 == lines.len();
            match serde_json::from_str(line) {
                Ok(record) => {
                    records.push(record);
                    unfinished_whole = last_unfinished;
                }
                Err(_) if last_unfinished => {}
                Err(source) => {
                    return Err(LedgerError::Malformed {
                        path,
                        line: index + 1,
                        source,
                    });
                }
            }
        }
        if !complete {
            finish_last_line(&path, &text, unfinished_whole).map_err(io_error)?;
        }
        Ok(Self { path, records })
    }

    /// Appends `record` as one line.
    pub fn append(&mut self, record: ProcessRecord) -> Result<(), LedgerError> {
        let mut line = serde_json::to_string(&record).map_err(|source| LedgerError::Malformed {
            path: self.path.clone(),
            line: self.records.len() + 1,
            source,
        })?;
        line.push('\n');
        let io_error = |source| LedgerError::Io {
            path: self.path.clone(),
            source,
        };
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(io_error)?;
        file.write_all(line.as_bytes()).map_err(io_error)?;
        self.records.push(record);
        Ok(())
    }

    /// Drops the records finished before `keep_since`, rewriting the file only when any is.
    pub fn prune(&mut self, keep_since: OffsetDateTime) -> Result<(), LedgerError> {
        let before = self.records.len();
        self.records.retain(|record| record.finished >= keep_since);
        if self.records.len() == before {
            return Ok(());
        }
        let io_error = |source| LedgerError::Io {
            path: self.path.clone(),
            source,
        };
        let temporary = self.path.with_extension("jsonl.tmp");
        let mut file = File::create(&temporary).map_err(io_error)?;
        for record in &self.records {
            let line = serde_json::to_string(record).map_err(|source| LedgerError::Malformed {
                path: self.path.clone(),
                line: 0,
                source,
            })?;
            writeln!(file, "{line}").map_err(io_error)?;
        }
        file.sync_all().map_err(io_error)?;
        std::fs::rename(&temporary, &self.path).map_err(io_error)
    }

    /// The records finished at or after `since`, oldest first.
    pub fn since(&self, since: OffsetDateTime) -> impl Iterator<Item = &ProcessRecord> {
        self.records
            .iter()
            .filter(move |record| record.finished >= since)
    }

    /// When the last process of each review in the ledger finished.
    pub fn last_worked(&self) -> BTreeMap<CrawlerReviewId, OffsetDateTime> {
        let mut last = BTreeMap::new();
        for record in &self.records {
            last.entry(record.review_id)
                .and_modify(|finished: &mut OffsetDateTime| {
                    *finished = (*finished).max(record.finished);
                })
                .or_insert(record.finished);
        }
        last
    }

    /// The `total_cost_usd` of the processes that finished within `window` before `now`.
    pub fn cost_within(&self, window: Duration, now: OffsetDateTime) -> f64 {
        self.since(now - window)
            .map(|record| record.total_cost_usd)
            .sum()
    }

    /// The highest cost a process in the ledger reported, the runner's own charges left out; zero
    /// when none reported one.
    pub fn highest_reported_cost(&self) -> f64 {
        self.records
            .iter()
            .filter(|record| !record.cost_charged)
            .map(|record| record.total_cost_usd)
            .fold(0.0, f64::max)
    }
}

/// Ends the ledger `text` at `path` with a newline: after its unfinished last line when that line
/// is a whole record, else in place of that line.
fn finish_last_line(path: &Path, text: &str, keep_last_line: bool) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).open(path)?;
    if keep_last_line {
        file.seek(SeekFrom::End(0))?;
        file.write_all(b"\n")?;
    } else {
        let kept = text.rfind('\n').map_or(0, |newline| newline + 1);
        file.set_len(u64::try_from(kept).unwrap_or(u64::MAX))?;
    }
    file.sync_all()
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use time::macros::datetime;

    /// A directory of its own for one test, removed when the test ends.
    pub(in crate::review_agent_runner) struct StateDirectory(PathBuf);

    impl StateDirectory {
        pub(in crate::review_agent_runner) fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "crawler-review-agent-runner-{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir(&path).unwrap_or_else(|error| panic!("state directory: {error}"));
            Self(path)
        }

        pub(in crate::review_agent_runner) fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for StateDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub(in crate::review_agent_runner) fn record(
        finished: OffsetDateTime,
        cost: f64,
    ) -> ProcessRecord {
        ProcessRecord {
            finished,
            review_id: CrawlerReviewId::new(),
            role: ProcessRole::Author,
            prompt_version: "author.v1".to_owned(),
            end: ProcessEnd::Reported,
            limit: None,
            usage_limit: None,
            failure: None,
            subtype: Some("success".to_owned()),
            terminal_reason: None,
            api_error_status: None,
            num_turns: 3,
            duration_ms: 1_000,
            models: vec!["claude-opus-5-5".to_owned()],
            usage: TokenUsage::default(),
            total_cost_usd: cost,
            cost_charged: false,
            report: None,
        }
    }

    const NOW: OffsetDateTime = datetime!(2026-10-28 12:00 UTC);

    #[test]
    fn should_sum_the_cost_of_the_rolling_windows_across_runs() {
        let directory = StateDirectory::new();
        let mut ledger = ProcessLedger::open(directory.path()).unwrap_or_else(|e| panic!("{e}"));
        for (hours_ago, cost) in [
            (1, 1.5),
            (4, 2.0),
            (6, 4.0),
            (24 * 6, 8.0),
            (24 * 7 + 1, 16.0),
        ] {
            ledger
                .append(record(NOW - Duration::hours(hours_ago), cost))
                .unwrap_or_else(|e| panic!("{e}"));
        }

        let reopened = ProcessLedger::open(directory.path()).unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(reopened.cost_within(FIVE_HOURS, NOW), 3.5);
        assert_eq!(reopened.cost_within(ONE_WEEK, NOW), 15.5);
    }

    #[test]
    fn should_find_the_highest_reported_cost_without_the_runner_s_charges() {
        let directory = StateDirectory::new();
        let mut ledger = ProcessLedger::open(directory.path()).unwrap_or_else(|e| panic!("{e}"));
        let empty = ledger.highest_reported_cost();
        for record in [
            record(NOW - Duration::hours(3), 1.5),
            record(NOW - Duration::hours(2), 4.0),
            ProcessRecord {
                cost_charged: true,
                ..record(NOW - Duration::hours(1), 9.0)
            },
        ] {
            ledger.append(record).unwrap_or_else(|e| panic!("{e}"));
        }

        let reopened = ProcessLedger::open(directory.path()).unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(empty, 0.0);
        assert_eq!(reopened.highest_reported_cost(), 4.0);
        assert_eq!(reopened.cost_within(FIVE_HOURS, NOW), 14.5);
    }

    #[test]
    fn should_prune_old_records_and_keep_the_rest_on_disk() {
        let directory = StateDirectory::new();
        let mut ledger = ProcessLedger::open(directory.path()).unwrap_or_else(|e| panic!("{e}"));
        let old = record(NOW - Duration::days(9), 1.0);
        let recent = record(NOW - Duration::days(1), 2.0);
        ledger.append(old).unwrap_or_else(|e| panic!("{e}"));
        ledger
            .append(recent.clone())
            .unwrap_or_else(|e| panic!("{e}"));

        ledger
            .prune(NOW - LEDGER_RETENTION)
            .unwrap_or_else(|e| panic!("{e}"));

        let reopened = ProcessLedger::open(directory.path()).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            reopened.since(NOW - Duration::days(30)).collect::<Vec<_>>(),
            [&recent]
        );
    }

    #[test]
    fn should_leave_out_an_unfinished_last_line_and_refuse_any_other_bad_line() {
        let directory = StateDirectory::new();
        let line = serde_json::to_string(&record(NOW, 1.0)).unwrap_or_default();
        let path = directory.path().join(LEDGER_FILE);

        std::fs::write(&path, format!("{line}\n{{\"finished\":\"20")).unwrap_or_default();
        let interrupted = ProcessLedger::open(directory.path()).map(|ledger| ledger.records.len());
        std::fs::write(&path, format!("not a record\n{line}\n")).unwrap_or_default();
        let corrupted = ProcessLedger::open(directory.path());

        assert_eq!(interrupted.ok(), Some(1));
        assert!(matches!(
            corrupted,
            Err(LedgerError::Malformed { line: 1, .. })
        ));
    }

    /// The run after an interrupted one appends to the ledger, and the run after that still reads
    /// it: the unfinished line never merges with the next record into a line that is no record.
    #[test]
    fn should_keep_the_ledger_readable_after_appending_to_an_interrupted_one() {
        let line = serde_json::to_string(&record(NOW, 1.0)).unwrap_or_default();
        for interrupted in [format!("{line}\n{{\"finished\":\"20"), line.clone()] {
            let directory = StateDirectory::new();
            std::fs::write(directory.path().join(LEDGER_FILE), &interrupted).unwrap_or_default();

            let mut ledger =
                ProcessLedger::open(directory.path()).unwrap_or_else(|e| panic!("{e}"));
            ledger
                .append(record(NOW, 4.0))
                .unwrap_or_else(|e| panic!("{e}"));
            let reopened = ProcessLedger::open(directory.path()).unwrap_or_else(|e| panic!("{e}"));

            assert_eq!(
                reopened.cost_within(ONE_WEEK, NOW + Duration::minutes(1)),
                5.0,
                "{interrupted}"
            );
        }
    }
}
