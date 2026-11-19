//! What the agents did when they could not finish a review or qualification in their regular
//! rounds: each final round's outcome, each source they parked and each expired park they sent back
//! to qualification. The runner records them in `parking.jsonl` in its state directory, which the
//! digest reads, and asks the console to retry the expired parks before each run's qualification
//! pass (`docs/crawler/operations.md`, review agent runner).

use crate::CrawlerReviewId;
use crate::review::model::RetriedParkedSource;
use crate::review_mcp::{ConsoleClient, ConsoleError};
use listing_source_core::ListingSourceId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::{self, BufRead, Write as _};
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use tracing::{info, warn};

/// The file in the runner's state directory that records the final rounds, parks and retries.
pub const PARKING_LEDGER_FILE: &str = "parking.jsonl";

/// The console's route that sends the expired parks back to qualification.
const RETRY_PARKED_ROUTE: &str = "/api/source-qualifications/retry-parked";

/// How a final round on a review ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FinalRoundOutcome {
    /// The final-round reviewer approved the review.
    Approved,
    /// The final-round reviewer rejected it.
    Rejected,
    /// The source was parked with a label, by the final-round author or by the runner.
    Parked,
    /// The round stopped for a usage limit, a pause or the runner's stop; the next run goes on with
    /// it.
    Open,
    /// It ended without approval and the runner could not park the source; the review waits for a
    /// human, which the human queue report shows.
    NotParked,
}

/// One entry of the parking ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ParkingEvent {
    /// A final round on a review ended, or stopped.
    FinalRound {
        review_id: CrawlerReviewId,
        artifact_type: String,
        outcome: FinalRoundOutcome,
        /// The label the source was parked with.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        /// Whether the park was not the source's first since a human labelled it, so it waits
        /// longer than the first interval.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        parked_again: bool,
    },
    /// A final round on a source's qualification decided, or left it undecided.
    FinalQualification { decided: Option<String> },
    /// An expired park was sent back to qualification.
    Retried { label: String, requalifies: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParkingRecord {
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: String,
    #[serde(flatten)]
    pub event: ParkingEvent,
}

#[derive(Debug, thiserror::Error)]
#[error("cannot write the parking ledger {}", path.display())]
pub struct ParkingLedgerError {
    path: PathBuf,
    #[source]
    source: io::Error,
}

/// Appends `record` to the parking ledger in `state_dir`.
pub fn append(state_dir: &Path, record: &ParkingRecord) -> Result<(), ParkingLedgerError> {
    let path = state_dir.join(PARKING_LEDGER_FILE);
    let failed = |source| ParkingLedgerError {
        path: path.clone(),
        source,
    };
    let line = serde_json::to_string(record).map_err(|error| failed(io::Error::other(error)))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(failed)?;
    writeln!(file, "{line}").map_err(failed)
}

/// Appends `record`, logging a ledger that cannot be written: the console already holds what the
/// record tells, so the run goes on.
pub fn record(state_dir: &Path, record: &ParkingRecord) {
    if let Err(error) = append(state_dir, record) {
        warn!(
            error = %error,
            cause = ?std::error::Error::source(&error),
            "Cannot record a final round or park in the parking ledger; the digest misses it"
        );
    }
}

/// The records at or after `since`; a line that is no record is skipped with a warning.
pub fn records_since(state_dir: &Path, since: OffsetDateTime) -> Vec<ParkingRecord> {
    let path = state_dir.join(PARKING_LEDGER_FILE);
    let Ok(file) = std::fs::File::open(&path) else {
        return Vec::new();
    };
    io::BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| match serde_json::from_str::<ParkingRecord>(&line) {
            Ok(record) => Some(record),
            Err(error) => {
                warn!(path = %path.display(), error = %error, "Skipping an unreadable parking record");
                None
            }
        })
        .filter(|record| record.at >= since)
        .collect()
}

/// Drops the records before `before`, as the other ledgers keep only their retention.
pub fn prune(state_dir: &Path, before: OffsetDateTime) -> Result<(), ParkingLedgerError> {
    let path = state_dir.join(PARKING_LEDGER_FILE);
    let failed = |source| ParkingLedgerError {
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

/// Asks the console to send the sources whose agent park label expired back to qualification, and
/// records and logs each. The console clears at most a fixed number per call; the rest follow on
/// the next run.
pub async fn retry_expired_parks(
    console: &ConsoleClient,
    state_dir: &Path,
) -> Result<Vec<RetriedParkedSource>, ConsoleError> {
    #[derive(Deserialize)]
    struct Answer {
        retried: Vec<RetriedParkedSource>,
    }
    let answer: Answer = console
        .post(RETRY_PARKED_ROUTE, &serde_json::json!({}))
        .await?
        .decode()?;
    let now = OffsetDateTime::now_utc();
    for source in &answer.retried {
        record(
            state_dir,
            &ParkingRecord {
                at: now,
                listing_source_id: source.listing_source_id,
                listing_source_name: source.listing_source_name.clone(),
                event: ParkingEvent::Retried {
                    label: source.label.as_str().to_owned(),
                    requalifies: source.requalifies,
                },
            },
        );
    }
    if !answer.retried.is_empty() {
        let mut by_label: BTreeMap<&str, usize> = BTreeMap::new();
        for source in &answer.retried {
            *by_label.entry(source.label.as_str()).or_default() += 1;
        }
        info!(
            event = "crawler.review_agent.parks_retried",
            retried = answer.retried.len(),
            requalify = answer.retried.iter().filter(|source| source.requalifies).count(),
            by_label = ?by_label,
            "Parks expired; the agents try their sources again from qualification"
        );
    }
    Ok(answer.retried)
}

/// How many final rounds, of reviews and qualifications, the ledger records at or after `since`:
/// each started round once, however many runs it took.
pub fn final_rounds_since(records: &[ParkingRecord], since: OffsetDateTime) -> usize {
    records
        .iter()
        .filter(|record| record.at >= since)
        .filter(|record| match &record.event {
            ParkingEvent::FinalRound { outcome, .. } => *outcome != FinalRoundOutcome::Open,
            ParkingEvent::FinalQualification { .. } => true,
            ParkingEvent::Retried { .. } => false,
        })
        .count()
}

/// The digest's section on the agents' final rounds, parks and retries in `records`, and of the
/// agents' approvals in the window (`approved_sources`) those of sources a retry brought back.
pub fn digest_section(records: &[ParkingRecord], approved_sources: &[ListingSourceId]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "## Final rounds and parked sources\n");
    if records.is_empty() {
        let _ = writeln!(out, "No final round ran and no park expired.\n");
        return out;
    }
    let mut outcomes: BTreeMap<String, usize> = BTreeMap::new();
    let mut parked_by_label: BTreeMap<&str, usize> = BTreeMap::new();
    let mut parked_again = 0;
    let mut qualifications: BTreeMap<&str, usize> = BTreeMap::new();
    let mut retried_by_label: BTreeMap<&str, usize> = BTreeMap::new();
    let mut retried_sources = BTreeSet::new();
    for record in records {
        match &record.event {
            ParkingEvent::FinalRound {
                outcome,
                label,
                parked_again: again,
                ..
            } => {
                *outcomes
                    .entry(
                        serde_json::to_value(outcome)
                            .ok()
                            .and_then(|value| value.as_str().map(str::to_owned))
                            .unwrap_or_default(),
                    )
                    .or_default() += 1;
                if let Some(label) = label {
                    *parked_by_label.entry(label).or_default() += 1;
                }
                parked_again += usize::from(*again);
            }
            ParkingEvent::FinalQualification { decided } => {
                *qualifications
                    .entry(decided.as_deref().unwrap_or("NOT_DECIDED"))
                    .or_default() += 1;
            }
            ParkingEvent::Retried { label, .. } => {
                *retried_by_label.entry(label).or_default() += 1;
                retried_sources.insert(record.listing_source_id);
            }
        }
    }
    let came_back = approved_sources
        .iter()
        .filter(|source| retried_sources.contains(source))
        .count();
    let join = |counts: Vec<(String, usize)>| {
        if counts.is_empty() {
            "none".to_owned()
        } else {
            counts
                .into_iter()
                .map(|(name, count)| format!("{name} {count}"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    };
    let _ = writeln!(
        out,
        "- Final rounds on reviews: {}",
        join(outcomes.into_iter().collect())
    );
    let _ = writeln!(
        out,
        "- Sources parked: {} ({parked_again} parked again)",
        join(
            parked_by_label
                .into_iter()
                .map(|(label, count)| (label.to_owned(), count))
                .collect()
        )
    );
    let _ = writeln!(
        out,
        "- Final rounds on qualifications: {}",
        join(
            qualifications
                .into_iter()
                .map(|(decided, count)| (decided.to_owned(), count))
                .collect()
        )
    );
    let _ = writeln!(
        out,
        "- Parks expired and retried: {}",
        join(
            retried_by_label
                .into_iter()
                .map(|(label, count)| (label.to_owned(), count))
                .collect()
        )
    );
    let _ = writeln!(
        out,
        "- Retried sources whose review the agents approved since: {came_back}\n"
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(minutes: i64) -> OffsetDateTime {
        OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000) + time::Duration::minutes(minutes)
    }

    fn final_round(
        outcome: FinalRoundOutcome,
        label: Option<&str>,
        parked_again: bool,
    ) -> ParkingRecord {
        ParkingRecord {
            at: at(0),
            listing_source_id: ListingSourceId::new(),
            listing_source_name: "Shop".to_owned(),
            event: ParkingEvent::FinalRound {
                review_id: CrawlerReviewId::new(),
                artifact_type: "PRODUCT_SCHEMA".to_owned(),
                outcome,
                label: label.map(str::to_owned),
                parked_again,
            },
        }
    }

    #[test]
    fn should_count_each_finished_final_round_once_against_the_daily_cap() {
        let records = vec![
            final_round(FinalRoundOutcome::Approved, None, false),
            final_round(FinalRoundOutcome::Open, None, false),
            final_round(FinalRoundOutcome::Parked, Some("AGENTS_UNRESOLVED"), false),
            ParkingRecord {
                event: ParkingEvent::FinalQualification { decided: None },
                ..final_round(FinalRoundOutcome::Open, None, false)
            },
            ParkingRecord {
                event: ParkingEvent::Retried {
                    label: "BOT_PROTECTION".to_owned(),
                    requalifies: true,
                },
                ..final_round(FinalRoundOutcome::Open, None, false)
            },
        ];

        assert_eq!(final_rounds_since(&records, at(0)), 3);
        assert_eq!(final_rounds_since(&records, at(1)), 0);
    }

    #[test]
    fn should_round_trip_a_record_through_the_ledger_and_prune_it() {
        let dir = std::env::temp_dir().join(format!("parking-ledger-{}", ListingSourceId::new()));
        std::fs::create_dir_all(&dir).unwrap_or_else(|error| panic!("{error}"));
        let parked = final_round(FinalRoundOutcome::Parked, Some("BOT_PROTECTION"), true);

        append(&dir, &parked).unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(records_since(&dir, at(0)), vec![parked]);
        prune(&dir, at(1)).unwrap_or_else(|error| panic!("{error}"));
        assert!(records_since(&dir, at(0)).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn should_report_outcomes_parks_retries_and_sources_that_came_back() {
        let retried = ParkingRecord {
            event: ParkingEvent::Retried {
                label: "BOT_PROTECTION".to_owned(),
                requalifies: true,
            },
            ..final_round(FinalRoundOutcome::Open, None, false)
        };
        let records = vec![
            final_round(FinalRoundOutcome::Approved, None, false),
            final_round(FinalRoundOutcome::Parked, Some("AGENTS_UNRESOLVED"), true),
            final_round(
                FinalRoundOutcome::Parked,
                Some("URL_PATTERN_APPROACH_NOT_APPLICABLE"),
                false,
            ),
            ParkingRecord {
                event: ParkingEvent::FinalQualification {
                    decided: Some("QUALIFIED".to_owned()),
                },
                ..final_round(FinalRoundOutcome::Open, None, false)
            },
            retried.clone(),
        ];

        let section = digest_section(&records, &[retried.listing_source_id]);

        for expected in [
            "- Final rounds on reviews: APPROVED 1, PARKED 2",
            "- Sources parked: AGENTS_UNRESOLVED 1, URL_PATTERN_APPROACH_NOT_APPLICABLE 1 (1 parked again)",
            "- Final rounds on qualifications: QUALIFIED 1",
            "- Parks expired and retried: BOT_PROTECTION 1",
            "- Retried sources whose review the agents approved since: 1",
        ] {
            assert!(section.contains(expected), "{expected}\n{section}");
        }
    }
}
