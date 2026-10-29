//! The daily digest of the agents' work, for humans who oversee after the fact: the reviews that
//! wait for a human because the agents' needs-repair rounds on them reached the cap, what the
//! event trail of all reviews recorded in the window (approvals, rejections and needs-repair
//! verdicts by actor, restores and revoked rejections), what the agents reported (the server's
//! refusals by failed check, each process's outcome), and each review's tokens and cost. It is
//! written to a Markdown file in the state directory's `digests/` and summarised in one log line.

use super::envelope::TokenUsage;
use super::ledger::{ProcessEnd, ProcessRecord, ProcessRole};
use super::queue::{HumanRequiredReview, ReviewEventEntry, ReviewEventsSince};
use super::reports::{AgentReport, ServerOutcome};
use super::wire_name;
use crate::CrawlerReviewId;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

pub const DIGEST_DIR: &str = "digests";
/// The longest note or summary the digest shows, in characters.
const MAX_NOTE_CHARS: usize = 300;
const AGENT_ACTORS: [&str; 2] = ["AGENT_AUTHOR", "AGENT_REVIEWER"];

/// The decisions of one action in the window, by agents and by everyone else.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecisionCount {
    pub agents: usize,
    pub others: usize,
}

/// One review's agent processes in the window.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReviewWork {
    pub processes: Vec<String>,
    pub usage: TokenUsage,
    pub cost_usd: f64,
}

/// The rolling cost sums and the pause at the digest's time.
#[derive(Debug, Clone, PartialEq)]
pub struct RollingCost {
    pub five_hour_cost_usd: f64,
    pub weekly_cost_usd: f64,
    pub paused_until: Option<(OffsetDateTime, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Digest {
    pub from: OffsetDateTime,
    pub to: OffsetDateTime,
    pub approved: DecisionCount,
    pub rejected: DecisionCount,
    pub needs_repair: DecisionCount,
    pub restored: usize,
    /// The rejections a human revoked, so they classify their pages as non-product pages no more.
    pub revoked_rejections: usize,
    /// The pending reviews that wait for a human at the digest's time.
    pub needs_a_human: Vec<HumanRequiredReview>,
    decisions: Vec<ReviewEventEntry>,
    events_truncated: bool,
    /// The server's refusals of the reviewer's decisions by failed check, as the reports name them.
    pub refusals_by_check: BTreeMap<String, usize>,
    /// Refusals that named no failed check, by their kind.
    pub refusals_by_kind: BTreeMap<String, usize>,
    pub processes_by_end: BTreeMap<&'static str, usize>,
    pub usage: TokenUsage,
    pub cost_usd: f64,
    pub reviews: BTreeMap<CrawlerReviewId, ReviewWork>,
    reports: Vec<ProcessRecord>,
    rolling: RollingCost,
}

impl Digest {
    /// The digest of `[from, to]` from the console's `events` since `from`, the ledger's
    /// `records` of the window and the reviews `needs_a_human` that wait for a human at `to`.
    pub fn of(
        from: OffsetDateTime,
        to: OffsetDateTime,
        events: ReviewEventsSince,
        records: Vec<ProcessRecord>,
        needs_a_human: Vec<HumanRequiredReview>,
        rolling: RollingCost,
    ) -> Self {
        let decisions: Vec<ReviewEventEntry> = events
            .events
            .into_iter()
            .filter(|event| event.created <= to)
            .filter(|event| {
                matches!(
                    event.action.as_str(),
                    "APPROVED" | "REJECTED" | "REPAIR_REQUESTED" | "RESTORED" | "REJECTION_REVOKED"
                )
            })
            .collect();
        let count = |action: &str| {
            decisions
                .iter()
                .filter(|event| event.action == action)
                .fold(DecisionCount::default(), |mut count, event| {
                    if AGENT_ACTORS.contains(&event.actor.as_str()) {
                        count.agents += 1;
                    } else {
                        count.others += 1;
                    }
                    count
                })
        };
        let total = |action: &str| {
            decisions
                .iter()
                .filter(|event| event.action == action)
                .count()
        };
        let mut digest = Self {
            from,
            to,
            approved: count("APPROVED"),
            rejected: count("REJECTED"),
            needs_repair: count("REPAIR_REQUESTED"),
            restored: total("RESTORED"),
            revoked_rejections: total("REJECTION_REVOKED"),
            needs_a_human,
            decisions,
            events_truncated: events.truncated,
            refusals_by_check: BTreeMap::new(),
            refusals_by_kind: BTreeMap::new(),
            processes_by_end: BTreeMap::new(),
            usage: TokenUsage::default(),
            cost_usd: 0.0,
            reviews: BTreeMap::new(),
            reports: Vec::new(),
            rolling,
        };
        for record in records
            .into_iter()
            .filter(|record| record.finished >= from && record.finished <= to)
        {
            digest.add_process(record);
        }
        digest
    }

    fn add_process(&mut self, record: ProcessRecord) {
        *self
            .processes_by_end
            .entry(end_name(record.end))
            .or_default() += 1;
        self.usage.add(record.usage);
        self.cost_usd += record.total_cost_usd;
        if let Some(AgentReport::Reviewer(report)) = &record.report
            && report.server_answer.outcome == ServerOutcome::Refused
        {
            if report.server_answer.failed_checks.is_empty() {
                let kind = if report.server_answer.kind.is_empty() {
                    "UNNAMED".to_owned()
                } else {
                    report.server_answer.kind.clone()
                };
                *self.refusals_by_kind.entry(kind).or_default() += 1;
            }
            for check in &report.server_answer.failed_checks {
                *self.refusals_by_check.entry(check.clone()).or_default() += 1;
            }
        }
        let work = self.reviews.entry(record.review_id).or_default();
        work.processes.push(process_line(&record));
        work.usage.add(record.usage);
        work.cost_usd += record.total_cost_usd;
        if record.report.is_some() {
            self.reports.push(record);
        }
    }

    /// The digest as the Markdown file the operator reads.
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# Review agents digest\n");
        let _ = writeln!(
            out,
            "Window: {} to {} (UTC).\n",
            timestamp(self.from),
            timestamp(self.to)
        );
        self.write_needs_a_human(&mut out);
        self.write_decisions(&mut out);
        self.write_refusals(&mut out);
        self.write_processes(&mut out);
        self.write_reports(&mut out);
        self.write_rolling_cost(&mut out);
        out
    }

    fn write_needs_a_human(&self, out: &mut String) {
        let _ = writeln!(out, "## Needs a human\n");
        if self.needs_a_human.is_empty() {
            let _ = writeln!(out, "None.\n");
            return;
        }
        let _ = writeln!(
            out,
            "The agents' needs-repair rounds on these pending reviews reached the cap, so no agent works them. Change or judge each in the review console to hand it back to the agents, or approve or reject it.\n"
        );
        for review in &self.needs_a_human {
            let _ = writeln!(
                out,
                "- {} {} · {} · {} agent rounds{}",
                review.review_id,
                review.artifact_type,
                review
                    .listing_source_name
                    .as_deref()
                    .unwrap_or("(unnamed source)"),
                review.agent_rounds,
                review
                    .repair_request
                    .as_ref()
                    .map(|repair| format!(
                        " · last needs-repair at {}: {}",
                        timestamp(repair.requested),
                        short(&repair.note)
                    ))
                    .unwrap_or_default()
            );
        }
        out.push('\n');
    }

    fn write_decisions(&self, out: &mut String) {
        let _ = writeln!(out, "## Decisions in the event trail\n");
        let _ = writeln!(out, "| Action | By agents | By humans or the crawler |");
        let _ = writeln!(out, "|---|---:|---:|");
        for (name, count) in [
            ("Approved", self.approved),
            ("Rejected", self.rejected),
            ("Needs repair", self.needs_repair),
        ] {
            let _ = writeln!(out, "| {name} | {} | {} |", count.agents, count.others);
        }
        let _ = writeln!(out, "| Restored | – | {} |", self.restored);
        let _ = writeln!(
            out,
            "| Rejection revoked | – | {} |\n",
            self.revoked_rejections
        );
        if self.events_truncated {
            let _ = writeln!(
                out,
                "Only the first events of the window were read; later ones are missing.\n"
            );
        }
        for event in &self.decisions {
            let _ = writeln!(
                out,
                "- {} · {} {} · {} · {} {} at version {}{}",
                timestamp(event.created),
                event.review_id,
                event.artifact_type,
                event
                    .listing_source_name
                    .as_deref()
                    .unwrap_or("(unnamed source)"),
                event.actor,
                event.action,
                event.candidate_version,
                event
                    .notes
                    .as_deref()
                    .map(|notes| format!(": {}", short(notes)))
                    .unwrap_or_default()
            );
        }
        if !self.decisions.is_empty() {
            out.push('\n');
        }
    }

    fn write_refusals(&self, out: &mut String) {
        let _ = writeln!(out, "## Server refusals of the reviewer's decisions\n");
        if self.refusals_by_check.is_empty() && self.refusals_by_kind.is_empty() {
            let _ = writeln!(out, "None reported.\n");
            return;
        }
        let _ = writeln!(out, "| Failed check or kind | Count |");
        let _ = writeln!(out, "|---|---:|");
        for (name, count) in self.refusals_by_check.iter().chain(&self.refusals_by_kind) {
            let _ = writeln!(out, "| {name} | {count} |");
        }
        out.push('\n');
    }

    fn write_processes(&self, out: &mut String) {
        let _ = writeln!(out, "## Agent processes\n");
        let ends: Vec<String> = self
            .processes_by_end
            .iter()
            .map(|(end, count)| format!("{count} {end}"))
            .collect();
        let _ = writeln!(
            out,
            "{} processes ({}); {} input, {} output, {} cache-read and {} cache-write tokens; ${:.2}.\n",
            self.processes_by_end.values().sum::<usize>(),
            if ends.is_empty() {
                "none".to_owned()
            } else {
                ends.join(", ")
            },
            self.usage.input_tokens,
            self.usage.output_tokens,
            self.usage.cache_read_input_tokens,
            self.usage.cache_creation_input_tokens,
            self.cost_usd
        );
        if self.reviews.is_empty() {
            return;
        }
        let _ = writeln!(
            out,
            "| Review | Processes | Input tokens | Output tokens | Cost |"
        );
        let _ = writeln!(out, "|---|---|---:|---:|---:|");
        for (review_id, work) in &self.reviews {
            let _ = writeln!(
                out,
                "| {review_id} | {} | {} | {} | ${:.2} |",
                work.processes.join("; "),
                work.usage.input_tokens,
                work.usage.output_tokens,
                work.cost_usd
            );
        }
        out.push('\n');
    }

    fn write_reports(&self, out: &mut String) {
        if self.reports.is_empty() {
            return;
        }
        let _ = writeln!(out, "## Reports\n");
        for record in &self.reports {
            let Some(report) = &record.report else {
                continue;
            };
            let line = match report {
                AgentReport::Author(report) => format!(
                    "author {} at version {}: {}{}",
                    wire_name(Some(report.action)),
                    report.candidate_version,
                    short(&report.summary),
                    risks(&report.risks)
                ),
                AgentReport::Reviewer(report) => format!(
                    "reviewer {} ({} confidence, server {}{}): {}{}",
                    wire_name(Some(report.decision)),
                    wire_name(Some(report.confidence)),
                    wire_name(Some(report.server_answer.outcome)),
                    if report.server_answer.failed_checks.is_empty() {
                        String::new()
                    } else {
                        format!(" {}", report.server_answer.failed_checks.join(", "))
                    },
                    short(&report.summary),
                    risks(&report.risks)
                ),
            };
            let _ = writeln!(
                out,
                "- {} · {} · {line}",
                timestamp(record.finished),
                record.review_id
            );
        }
        out.push('\n');
    }

    fn write_rolling_cost(&self, out: &mut String) {
        let rolling = &self.rolling;
        let _ = writeln!(out, "## Usage\n");
        let _ = writeln!(
            out,
            "Rolling five hours: ${:.2}. Rolling week: ${:.2}.",
            rolling.five_hour_cost_usd, rolling.weekly_cost_usd
        );
        match &rolling.paused_until {
            Some((until, limit)) => {
                let _ = writeln!(out, "Paused until {} ({limit}).", timestamp(*until));
            }
            None => {
                let _ = writeln!(out, "Not paused.");
            }
        }
    }

    /// Writes the digest to `digests/<to>.md` in `state_dir` and answers the file's path.
    pub fn write(&self, state_dir: &Path) -> std::io::Result<PathBuf> {
        let directory = state_dir.join(DIGEST_DIR);
        std::fs::create_dir_all(&directory)?;
        let name = timestamp(self.to).replace(':', "-");
        let path = directory.join(format!("review-agents-{name}.md"));
        std::fs::write(&path, self.markdown())?;
        Ok(path)
    }
}

fn end_name(end: ProcessEnd) -> &'static str {
    match end {
        ProcessEnd::Reported => "reported",
        ProcessEnd::LimitReached => "limit reached",
        ProcessEnd::UsageLimit => "usage limit",
        ProcessEnd::Failed => "failed",
    }
}

/// One process in a review's row: its role and how it ended.
fn process_line(record: &ProcessRecord) -> String {
    let role = match record.role {
        ProcessRole::Author => "author",
        ProcessRole::Reviewer => "reviewer",
    };
    let detail = match (&record.report, record.limit, &record.failure) {
        (Some(AgentReport::Author(report)), _, _) => wire_name(Some(report.action)),
        (Some(AgentReport::Reviewer(report)), _, _) => format!(
            "{} {}",
            wire_name(Some(report.decision)),
            wire_name(Some(report.server_answer.outcome))
        ),
        (None, Some(limit), _) => format!("{} limit", wire_name(Some(limit))),
        (None, None, Some(failure)) => short(failure),
        (None, None, None) => end_name(record.end).to_owned(),
    };
    let charged = if record.cost_charged {
        ", cost charged"
    } else {
        ""
    };
    format!("{role} {detail}{charged}")
}

fn risks(risks: &[String]) -> String {
    if risks.is_empty() {
        String::new()
    } else {
        format!(
            " Risks: {}",
            risks
                .iter()
                .map(|risk| short(risk))
                .collect::<Vec<_>>()
                .join("; ")
        )
    }
}

/// A one-line, shortened text for the digest's lists and tables.
fn short(text: &str) -> String {
    let line: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('|', "/");
    if line.chars().count() <= MAX_NOTE_CHARS {
        line
    } else {
        format!("{}…", line.chars().take(MAX_NOTE_CHARS).collect::<String>())
    }
}

/// `at` in RFC 3339, to the second.
pub(super) fn timestamp(at: OffsetDateTime) -> String {
    at.replace_nanosecond(0)
        .unwrap_or(at)
        .format(&Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::RepairRequest;
    use crate::review_agent_runner::ledger::tests::record;
    use crate::review_agent_runner::queue::ReviewArtifact;
    use crate::review_agent_runner::reports::{
        AuthorAction, AuthorReport, Confidence, ReviewerDecision, ReviewerReport, ServerAnswer,
    };
    use time::Duration;
    use time::macros::datetime;

    const TO: OffsetDateTime = datetime!(2026-10-28 06:00 UTC);
    const FROM: OffsetDateTime = datetime!(2026-10-27 06:00 UTC);

    fn event(actor: &str, action: &str, hours_before: i64) -> ReviewEventEntry {
        ReviewEventEntry {
            event_id: crate::CrawlerReviewEventId::new(),
            review_id: CrawlerReviewId::new(),
            artifact_type: "PRODUCT_SCHEMA".to_owned(),
            listing_source_name: Some("Antiques".to_owned()),
            actor: actor.to_owned(),
            action: action.to_owned(),
            candidate_version: 3,
            notes: Some("price | empty on\n/item/2".to_owned()),
            created: TO - Duration::hours(hours_before),
        }
    }

    fn reviewer(
        review_id: CrawlerReviewId,
        outcome: ServerOutcome,
        checks: &[&str],
    ) -> AgentReport {
        AgentReport::Reviewer(ReviewerReport {
            review_id: review_id.to_string(),
            decision: ReviewerDecision::Approve,
            confidence: Confidence::High,
            summary: "Checked every page.".to_owned(),
            risks: vec![],
            field_findings: vec![],
            server_answer: ServerAnswer {
                outcome,
                kind: if checks.is_empty() {
                    String::new()
                } else {
                    "CONFLICT".to_owned()
                },
                failed_checks: checks.iter().map(|check| (*check).to_owned()).collect(),
                detail: String::new(),
            },
        })
    }

    fn rolling_cost() -> RollingCost {
        RollingCost {
            five_hour_cost_usd: 1.5,
            weekly_cost_usd: 9.0,
            paused_until: None,
        }
    }

    #[test]
    fn should_count_the_window_s_decisions_by_actor_and_what_humans_undid() {
        let events = ReviewEventsSince {
            events: vec![
                event("AGENT_REVIEWER", "APPROVED", 20),
                event("AGENT_REVIEWER", "APPROVED", 10),
                event("HUMAN", "APPROVED", 9),
                event("AGENT_REVIEWER", "REJECTED", 8),
                event("AGENT_AUTHOR", "REPAIR_REQUESTED", 7),
                event("AGENT_REVIEWER", "REPAIR_REQUESTED", 6),
                event("HUMAN", "RESTORED", 5),
                event("AGENT_AUTHOR", "CANDIDATE_EDITED", 4),
                event("HUMAN", "REJECTION_REVOKED", 3),
            ],
            truncated: false,
        };

        let digest = Digest::of(FROM, TO, events, vec![], vec![], rolling_cost());

        assert_eq!(
            digest.approved,
            DecisionCount {
                agents: 2,
                others: 1
            }
        );
        assert_eq!(
            digest.rejected,
            DecisionCount {
                agents: 1,
                others: 0
            }
        );
        assert_eq!(
            digest.needs_repair,
            DecisionCount {
                agents: 2,
                others: 0
            }
        );
        assert_eq!(digest.restored, 1);
        assert_eq!(digest.revoked_rejections, 1);
        let markdown = digest.markdown();
        assert!(markdown.contains("| Approved | 2 | 1 |"), "{markdown}");
        assert!(markdown.contains("| Restored | – | 1 |"), "{markdown}");
        assert!(
            markdown.contains("| Rejection revoked | – | 1 |"),
            "{markdown}"
        );
        assert!(markdown.contains("HUMAN RESTORED at version 3: price / empty on /item/2"));
        assert!(markdown.contains("HUMAN REJECTION_REVOKED at version 3"));
        assert!(!markdown.contains("CANDIDATE_EDITED"));
    }

    #[test]
    fn should_sum_each_review_s_tokens_and_cost_and_the_refusals_by_failed_check() {
        let review_id = CrawlerReviewId::new();
        let at = TO - Duration::hours(2);
        let mut author = record(at, 1.25);
        author.review_id = review_id;
        author.usage.input_tokens = 1_000;
        author.report = Some(AgentReport::Author(AuthorReport {
            review_id: review_id.to_string(),
            action: AuthorAction::Staged,
            candidate_version: 4,
            summary: "Staged the pattern /item/.".to_owned(),
            risks: vec!["two labels only".to_owned()],
        }));
        let mut refused = record(at, 0.5);
        refused.review_id = review_id;
        refused.role = ProcessRole::Reviewer;
        refused.usage.input_tokens = 400;
        refused.report = Some(reviewer(
            review_id,
            ServerOutcome::Refused,
            &["TOO_FEW_LABELS", "PRODUCT_LABEL_NOT_MATCHED"],
        ));
        let mut limited = record(at, 2.0);
        limited.end = ProcessEnd::LimitReached;
        limited.limit = Some(crate::review_agent_runner::envelope::ProcessLimit::Turns);
        let outside = record(FROM - Duration::hours(1), 50.0);

        let digest = Digest::of(
            FROM,
            TO,
            ReviewEventsSince {
                events: vec![],
                truncated: true,
            },
            vec![author, refused, limited, outside],
            vec![],
            rolling_cost(),
        );

        assert_eq!(digest.cost_usd, 3.75);
        assert_eq!(digest.reviews[&review_id].cost_usd, 1.75);
        assert_eq!(digest.reviews[&review_id].usage.input_tokens, 1_400);
        assert_eq!(
            digest.reviews[&review_id].processes,
            ["author STAGED", "reviewer APPROVE REFUSED"]
        );
        assert_eq!(
            digest.refusals_by_check,
            BTreeMap::from([
                ("PRODUCT_LABEL_NOT_MATCHED".to_owned(), 1),
                ("TOO_FEW_LABELS".to_owned(), 1)
            ])
        );
        assert_eq!(
            digest.processes_by_end,
            BTreeMap::from([("limit reached", 1), ("reported", 2)])
        );
        let markdown = digest.markdown();
        assert!(markdown.contains("| TOO_FEW_LABELS | 1 |"), "{markdown}");
        assert!(markdown.contains("later ones are missing"), "{markdown}");
        assert!(markdown.contains(
            "author STAGED at version 4: Staged the pattern /item/. Risks: two labels only"
        ));
        assert!(markdown.contains("Rolling five hours: $1.50. Rolling week: $9.00."));
    }

    #[test]
    fn should_list_the_reviews_that_need_a_human_with_their_last_needs_repair_note() {
        let review_id = CrawlerReviewId::new();
        let waiting = HumanRequiredReview {
            review_id,
            artifact_type: ReviewArtifact::ProductSchema,
            listing_source_name: Some("Antiques".to_owned()),
            agent_rounds: 3,
            repair_request: Some(RepairRequest {
                note: "price | still empty on\n/item/2".to_owned(),
                requested: TO - Duration::hours(1),
            }),
        };
        let none = Digest::of(
            FROM,
            TO,
            ReviewEventsSince {
                events: vec![],
                truncated: false,
            },
            vec![],
            vec![],
            rolling_cost(),
        );

        let digest = Digest::of(
            FROM,
            TO,
            ReviewEventsSince {
                events: vec![],
                truncated: false,
            },
            vec![],
            vec![waiting],
            rolling_cost(),
        );

        assert!(
            none.markdown().contains("## Needs a human\n\nNone.\n"),
            "{}",
            none.markdown()
        );
        let markdown = digest.markdown();
        assert!(
            markdown.contains(&format!(
                "- {review_id} PRODUCT_SCHEMA · Antiques · 3 agent rounds · last needs-repair at 2026-10-28T05:00:00Z: price / still empty on /item/2\n"
            )),
            "{markdown}"
        );
        assert!(
            markdown.find("## Needs a human") < markdown.find("## Decisions in the event trail"),
            "{markdown}"
        );
    }

    #[test]
    fn should_write_the_digest_to_a_file_named_by_its_end() {
        let directory = crate::review_agent_runner::ledger::tests::StateDirectory::new();
        let digest = Digest::of(
            FROM,
            TO,
            ReviewEventsSince {
                events: vec![],
                truncated: false,
            },
            vec![],
            vec![],
            rolling_cost(),
        );

        let path = digest
            .write(directory.path())
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(
            path,
            directory
                .path()
                .join("digests/review-agents-2026-10-28T06-00-00Z.md")
        );
        assert!(
            std::fs::read_to_string(&path)
                .unwrap_or_default()
                .starts_with("# Review agents digest")
        );
    }
}
