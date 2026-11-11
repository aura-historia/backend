//! The daily digest of the agents' work, for humans who oversee after the fact: the reviews that
//! wait for a human (the agents' needs-repair rounds on them reached the cap, or an agent handed
//! them over), what the event trail of all reviews recorded in the window (approvals, rejections,
//! needs-repair verdicts, hand-overs and no-sold-page records by actor, restores and revoked
//! rejections), what the agents reported (the server's
//! refusals by failed check, each process's outcome), and each review's tokens and cost. It is
//! written to a Markdown file in the state directory's `digests/` and summarised in one log line.

use super::context_use::ContextUseSummary;
use super::envelope::TokenUsage;
use super::ledger::{ProcessEnd, ProcessRecord, ProcessRole};
use super::queue::{
    HumanRequiredReview, ReviewArtifact, ReviewEventEntry, ReviewEventsSince, ReviewQueue,
};
use super::reports::{AgentReport, ServerOutcome, TrailVerdict};
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

/// The product-schema reviews the agents can work at the digest's time and since when the oldest
/// of them waits: a source gets its schema review only after its pattern passed, and approving it
/// is what lets the source's products in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SchemaReviewWait {
    pub workable: usize,
    pub oldest_since: Option<OffsetDateTime>,
}

impl SchemaReviewWait {
    fn of(queue: &ReviewQueue) -> Self {
        let schema = || {
            queue
                .workable
                .iter()
                .filter(|review| review.artifact_type == ReviewArtifact::ProductSchema)
        };
        Self {
            workable: schema().count(),
            oldest_since: schema().map(|review| review.created).min(),
        }
    }
}

/// The agent processes of one role on one review type in the window: where the tokens go.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KindWork {
    pub processes: usize,
    /// Of them, those that hit their turn, budget or context limit before they decided.
    pub limit_reached: usize,
    pub turns: u64,
    pub usage: TokenUsage,
    pub cost_usd: f64,
}

/// The rolling cost sums and the pause at the digest's time.
#[derive(Debug, Clone, PartialEq)]
pub struct RollingCost {
    pub five_hour_cost_usd: f64,
    pub weekly_cost_usd: f64,
    pub paused_until: Option<(OffsetDateTime, String)>,
    /// Until when the runner starts no reviewer process, after the agents' decision cap.
    pub reviewers_paused_until: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Digest {
    pub from: OffsetDateTime,
    pub to: OffsetDateTime,
    pub approved: DecisionCount,
    pub rejected: DecisionCount,
    pub needs_repair: DecisionCount,
    /// The reviews an agent handed to a human at once, with its reasoning as the event's note.
    pub handed_to_human: DecisionCount,
    /// The records that no sold or unavailable page was found, on which an agent's approval may
    /// rest instead of a tested sold page, with the account of the search as the event's note.
    pub no_sold_page_recorded: DecisionCount,
    pub restored: usize,
    /// The rejections a human revoked, so they classify their pages as non-product pages no more.
    pub revoked_rejections: usize,
    /// The pending reviews that wait for a human at the digest's time.
    pub needs_a_human: Vec<HumanRequiredReview>,
    /// The agents' approvals of `NO_PATTERN` in the window, each of which keeps a ListingSource out
    /// of product crawling until it expires: the decisions a human spot-checks.
    pub agent_no_patterns: Vec<ReviewEventEntry>,
    pub schema_wait: SchemaReviewWait,
    decisions: Vec<ReviewEventEntry>,
    events_truncated: bool,
    /// The server's refusals of the reviewer's decisions by failed check, as the reports name them:
    /// the event trail records only what the console accepted, so a refusal is the agent's account.
    pub refusals_by_check: BTreeMap<String, usize>,
    /// Refusals that named no failed check, by their kind.
    pub refusals_by_kind: BTreeMap<String, usize>,
    pub processes_by_end: BTreeMap<&'static str, usize>,
    pub usage: TokenUsage,
    pub cost_usd: f64,
    pub reviews: BTreeMap<CrawlerReviewId, ReviewWork>,
    /// The processes of the window by role and review type, such as (`author`, `URL_PATTERN`);
    /// `UNKNOWN` for a record written before the runner recorded the type.
    pub by_kind: BTreeMap<(&'static str, String), KindWork>,
    context_use: ContextUseSummary,
    reports: Vec<ProcessRecord>,
    rolling: RollingCost,
}

impl Digest {
    /// The digest of `[from, to]` from the console's `events` since `from`, the ledger's
    /// `records` of the window and the agents' `queue` at `to`.
    pub fn of(
        from: OffsetDateTime,
        to: OffsetDateTime,
        events: ReviewEventsSince,
        records: Vec<ProcessRecord>,
        queue: ReviewQueue,
        rolling: RollingCost,
    ) -> Self {
        let decisions: Vec<ReviewEventEntry> = events
            .events
            .into_iter()
            .filter(|event| event.created <= to)
            .filter(|event| {
                matches!(
                    event.action.as_str(),
                    "APPROVED"
                        | "REJECTED"
                        | "REPAIR_REQUESTED"
                        | "HANDED_TO_HUMAN"
                        | "NO_SOLD_PAGE_RECORDED"
                        | "RESTORED"
                        | "REJECTION_REVOKED"
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
        let agent_no_patterns = decisions
            .iter()
            .filter(|event| {
                event.action == "APPROVED"
                    && AGENT_ACTORS.contains(&event.actor.as_str())
                    && event.approved_url_pattern_state.as_deref() == Some("NO_PATTERN")
            })
            .cloned()
            .collect();
        let schema_wait = SchemaReviewWait::of(&queue);
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
            handed_to_human: count("HANDED_TO_HUMAN"),
            no_sold_page_recorded: count("NO_SOLD_PAGE_RECORDED"),
            restored: total("RESTORED"),
            revoked_rejections: total("REJECTION_REVOKED"),
            needs_a_human: queue.waiting_for_human,
            agent_no_patterns,
            schema_wait,
            decisions,
            events_truncated: events.truncated,
            refusals_by_check: BTreeMap::new(),
            refusals_by_kind: BTreeMap::new(),
            processes_by_end: BTreeMap::new(),
            usage: TokenUsage::default(),
            cost_usd: 0.0,
            reviews: BTreeMap::new(),
            by_kind: BTreeMap::new(),
            context_use: ContextUseSummary::default(),
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
        let kind = self
            .by_kind
            .entry((
                role_name(record.role),
                record
                    .artifact_type
                    .clone()
                    .unwrap_or_else(|| "UNKNOWN".to_owned()),
            ))
            .or_default();
        kind.processes += 1;
        kind.limit_reached += usize::from(record.end == ProcessEnd::LimitReached);
        kind.turns += record.num_turns;
        kind.usage.add(record.usage);
        kind.cost_usd += record.total_cost_usd;
        self.context_use
            .add(role_name(record.role), &record.context);
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
        self.write_agent_no_patterns(&mut out);
        self.write_schema_wait(&mut out);
        self.write_decisions(&mut out);
        self.write_refusals(&mut out);
        self.write_processes(&mut out);
        self.context_use.write_markdown(&mut out);
        self.write_reports(&mut out);
        self.write_rolling_cost(&mut out);
        out
    }

    /// When the oldest pending review that waits for a human opened, if any waits.
    pub fn oldest_needs_a_human_opened(&self) -> Option<OffsetDateTime> {
        self.needs_a_human.iter().map(|review| review.created).min()
    }

    fn write_needs_a_human(&self, out: &mut String) {
        let _ = writeln!(out, "## Needs a human\n");
        let Some(oldest) = self.oldest_needs_a_human_opened() else {
            let _ = writeln!(out, "None.\n");
            return;
        };
        let _ = writeln!(
            out,
            "These pending reviews wait for a human: the agents' needs-repair rounds on them reached the cap, or an agent handed them over with its reasoning. No agent works them. Change or judge each in the review console to hand it back to the agents, or approve or reject it.\n"
        );
        let _ = writeln!(
            out,
            "{} review(s) wait for a human; the oldest opened at {} ({} hours ago).\n",
            self.needs_a_human.len(),
            timestamp(oldest),
            (self.to - oldest).whole_hours()
        );
        for review in &self.needs_a_human {
            let _ = writeln!(
                out,
                "- {} {} · {} · {} agent rounds{}{}",
                review.review_id,
                review.artifact_type,
                review
                    .listing_source_name
                    .as_deref()
                    .unwrap_or("(unnamed source)"),
                review.agent_rounds,
                self.handover_of(review.review_id),
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

    fn write_agent_no_patterns(&self, out: &mut String) {
        let _ = writeln!(out, "## Agent NO_PATTERN decisions to spot-check\n");
        if self.agent_no_patterns.is_empty() {
            let _ = writeln!(out, "None.\n");
            return;
        }
        let _ = writeln!(
            out,
            "Each keeps a ListingSource out of product crawling until it expires. Open a few in the review console and restore or change any that is wrong.\n"
        );
        for event in &self.agent_no_patterns {
            let _ = writeln!(
                out,
                "- {} · {} · {}{}",
                timestamp(event.created),
                event.review_id,
                event
                    .listing_source_name
                    .as_deref()
                    .unwrap_or("(unnamed source)"),
                event
                    .notes
                    .as_deref()
                    .map(|notes| format!(": {}", short(notes)))
                    .unwrap_or_default()
            );
        }
        out.push('\n');
    }

    fn write_schema_wait(&self, out: &mut String) {
        let _ = writeln!(out, "## Product-schema reviews waiting\n");
        let _ = writeln!(
            out,
            "{} product-schema review(s) the agents can work{}.\n",
            self.schema_wait.workable,
            self.schema_wait
                .oldest_since
                .map(|since| format!(
                    "; the oldest waits since {} ({} hours)",
                    timestamp(since),
                    (self.to - since).whole_hours()
                ))
                .unwrap_or_default()
        );
    }

    /// The latest hand-over of `review_id` in the window: who handed it over, when, and why.
    fn handover_of(&self, review_id: CrawlerReviewId) -> String {
        self.decisions
            .iter()
            .rev()
            .find(|event| event.review_id == review_id && event.action == "HANDED_TO_HUMAN")
            .map(|event| {
                format!(
                    " · handed over by {} at {}: {}",
                    event.actor,
                    timestamp(event.created),
                    short(event.notes.as_deref().unwrap_or("no reasoning recorded"))
                )
            })
            .unwrap_or_default()
    }

    fn write_decisions(&self, out: &mut String) {
        let _ = writeln!(out, "## Decisions in the event trail\n");
        let _ = writeln!(out, "| Action | By agents | By humans or the crawler |");
        let _ = writeln!(out, "|---|---:|---:|");
        for (name, count) in [
            ("Approved", self.approved),
            ("Rejected", self.rejected),
            ("Needs repair", self.needs_repair),
            ("Handed to a human", self.handed_to_human),
            ("No sold page recorded", self.no_sold_page_recorded),
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
        let _ = writeln!(out, "## Server refusals the reviewer reported\n");
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
        if !self.by_kind.is_empty() {
            let _ = writeln!(
                out,
                "| Role | Review type | Processes | Hit a limit | Turns per process | Input tokens | Cache-read tokens | Output tokens | Cost |"
            );
            let _ = writeln!(out, "|---|---|---:|---:|---:|---:|---:|---:|---:|");
            for ((role, artifact_type), kind) in &self.by_kind {
                let _ = writeln!(
                    out,
                    "| {role} | {artifact_type} | {} | {} | {:.1} | {} | {} | {} | ${:.2} |",
                    kind.processes,
                    kind.limit_reached,
                    kind.turns as f64 / kind.processes.max(1) as f64,
                    kind.usage.input_tokens,
                    kind.usage.cache_read_input_tokens,
                    kind.usage.output_tokens,
                    kind.cost_usd
                );
            }
            out.push('\n');
        }
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
                    "reviewer {} ({} confidence, {}, reported server answer {}{}): {}{}",
                    wire_name(Some(report.decision)),
                    wire_name(Some(report.confidence)),
                    trail_text(record.trail_verdict),
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
        if let Some(until) = rolling.reviewers_paused_until {
            let _ = writeln!(
                out,
                "Reviewers paused until {} (the agents' decision cap); authors keep running.",
                timestamp(until)
            );
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

fn role_name(role: ProcessRole) -> &'static str {
    match role {
        ProcessRole::Author => "author",
        ProcessRole::Reviewer => "reviewer",
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
    let role = role_name(record.role);
    let detail = match (&record.report, record.limit, &record.failure) {
        (Some(AgentReport::Author(report)), _, _) => wire_name(Some(report.action)),
        (Some(AgentReport::Reviewer(report)), _, _) => format!(
            "{} ({}, reported {})",
            wire_name(Some(report.decision)),
            trail_text(record.trail_verdict),
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

/// What the event trail recorded of a reviewer process, which the digest shows ahead of the
/// report's own claim about the server's answer. A record without it, from before the runner read
/// the trail or from a trail it could not read, says so.
fn trail_text(verdict: Option<TrailVerdict>) -> String {
    match verdict {
        Some(verdict) => format!("trail {}", wire_name(Some(verdict))),
        None => "trail unknown".to_owned(),
    }
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
    use crate::review_agent_runner::queue::{QueuedReview, ReviewArtifact};
    use crate::review_agent_runner::reports::{
        AuthorAction, AuthorReport, Confidence, ReviewerDecision, ReviewerReport, ServerAnswer,
        TrailVerdict,
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
            approved_url_pattern_state: None,
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

    /// The agents' `NO_PATTERN` approvals are listed for a spot check, a human's or a pattern's
    /// are not; and the digest tells how many product-schema reviews the agents can work and since
    /// when the oldest waits, leaving URL-pattern reviews out.
    #[test]
    fn should_list_agent_no_patterns_and_the_schema_review_wait() {
        let approval = |actor: &str, state: Option<&str>| ReviewEventEntry {
            artifact_type: "URL_PATTERN".to_owned(),
            approved_url_pattern_state: state.map(str::to_owned),
            ..event(actor, "APPROVED", 2)
        };
        let agent_no_pattern = approval("AGENT_REVIEWER", Some("NO_PATTERN"));
        let queued = |artifact_type, hours_before| QueuedReview {
            review_id: CrawlerReviewId::new(),
            listing_source_id: listing_source_core::ListingSourceId::new(),
            artifact_type,
            listing_source_name: None,
            handed_back: None,
            created: TO - Duration::hours(hours_before),
        };
        let queue = ReviewQueue {
            workable: vec![
                queued(ReviewArtifact::UrlPattern, 100),
                queued(ReviewArtifact::ProductSchema, 30),
                queued(ReviewArtifact::ProductSchema, 5),
            ],
            waiting_for_human: vec![],
        };

        let digest = Digest::of(
            FROM,
            TO,
            ReviewEventsSince {
                events: vec![
                    agent_no_pattern.clone(),
                    approval("HUMAN", Some("NO_PATTERN")),
                    approval("AGENT_REVIEWER", Some("MATCHED")),
                ],
                truncated: false,
            },
            vec![],
            queue,
            rolling_cost(),
        );

        assert_eq!(digest.agent_no_patterns, vec![agent_no_pattern.clone()]);
        assert_eq!(
            digest.schema_wait,
            SchemaReviewWait {
                workable: 2,
                oldest_since: Some(TO - Duration::hours(30)),
            }
        );
        let markdown = digest.markdown();
        assert!(
            markdown.contains(&agent_no_pattern.review_id.to_string())
                && markdown.contains("2 product-schema review(s) the agents can work; the oldest waits since 2026-10-27T00:00:00Z (30 hours)"),
            "{markdown}"
        );
    }

    fn rolling_cost() -> RollingCost {
        RollingCost {
            five_hour_cost_usd: 1.5,
            weekly_cost_usd: 9.0,
            paused_until: None,
            reviewers_paused_until: None,
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

        let digest = Digest::of(
            FROM,
            TO,
            events,
            vec![],
            ReviewQueue::default(),
            rolling_cost(),
        );

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
        author.artifact_type = Some("URL_PATTERN".to_owned());
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
        refused.artifact_type = Some("URL_PATTERN".to_owned());
        refused.usage.input_tokens = 400;
        refused.report = Some(reviewer(
            review_id,
            ServerOutcome::Refused,
            &["PATTERN_MATCHES_NO_SAMPLE_URL", "PATTERN_TAKES_PROBE_PATHS"],
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
            ReviewQueue::default(),
            rolling_cost(),
        );

        assert_eq!(digest.cost_usd, 3.75);
        let kind = |role: &'static str, artifact_type: &str| {
            digest.by_kind[&(role, artifact_type.to_owned())].clone()
        };
        assert_eq!(
            (
                kind("author", "URL_PATTERN").processes,
                kind("author", "URL_PATTERN").usage.input_tokens,
                kind("reviewer", "URL_PATTERN").cost_usd,
            ),
            (1, 1_000, 0.5)
        );
        assert_eq!(
            (
                kind("author", "UNKNOWN").processes,
                kind("author", "UNKNOWN").limit_reached
            ),
            (1, 1)
        );
        assert!(
            digest
                .markdown()
                .contains("| reviewer | URL_PATTERN | 1 | 0 | 3.0 | 400 |"),
            "{}",
            digest.markdown()
        );
        assert_eq!(digest.reviews[&review_id].cost_usd, 1.75);
        assert_eq!(digest.reviews[&review_id].usage.input_tokens, 1_400);
        assert_eq!(
            digest.reviews[&review_id].processes,
            [
                "author STAGED",
                "reviewer APPROVE (trail unknown, reported REFUSED)"
            ]
        );
        assert_eq!(
            digest.refusals_by_check,
            BTreeMap::from([
                ("PATTERN_TAKES_PROBE_PATHS".to_owned(), 1),
                ("PATTERN_MATCHES_NO_SAMPLE_URL".to_owned(), 1)
            ])
        );
        assert_eq!(
            digest.processes_by_end,
            BTreeMap::from([("limit reached", 1), ("reported", 2)])
        );
        let markdown = digest.markdown();
        assert!(
            markdown.contains("| PATTERN_MATCHES_NO_SAMPLE_URL | 1 |"),
            "{markdown}"
        );
        assert!(markdown.contains("later ones are missing"), "{markdown}");
        assert!(markdown.contains(
            "author STAGED at version 4: Staged the pattern /item/. Risks: two labels only"
        ));
        assert!(markdown.contains("Rolling five hours: $1.50. Rolling week: $9.00."));
    }

    /// The digest and the process rows show what the event trail recorded of a reviewer process,
    /// and the report's `NOT_SENT` only as the agent's reported account.
    #[test]
    fn should_show_the_trail_s_verdict_and_label_the_report_s_server_answer_as_reported() {
        let mut contradicted = record(TO - Duration::hours(1), 0.2);
        contradicted.role = ProcessRole::Reviewer;
        contradicted.trail_verdict = Some(TrailVerdict::Approved);
        contradicted.report = Some(reviewer(
            contradicted.review_id,
            ServerOutcome::NotSent,
            &[],
        ));
        let mut legacy = record(TO - Duration::hours(2), 0.2);
        legacy.role = ProcessRole::Reviewer;
        legacy.report = Some(reviewer(legacy.review_id, ServerOutcome::NotSent, &[]));

        let digest = Digest::of(
            FROM,
            TO,
            ReviewEventsSince {
                events: vec![],
                truncated: false,
            },
            vec![contradicted.clone(), legacy.clone()],
            ReviewQueue::default(),
            rolling_cost(),
        );

        let markdown = digest.markdown();
        assert!(
            markdown.contains("reviewer APPROVE (trail APPROVED, reported NOT_SENT)"),
            "{markdown}"
        );
        assert!(
            markdown.contains("reviewer APPROVE (trail unknown, reported NOT_SENT)"),
            "{markdown}"
        );
        assert!(
            markdown.contains(
                "reviewer APPROVE (HIGH confidence, trail APPROVED, reported server answer NOT_SENT)"
            ),
            "{markdown}"
        );
    }

    #[test]
    fn should_count_a_hand_over_and_show_its_reasoning_where_the_review_waits_for_a_human() {
        let review_id = CrawlerReviewId::new();
        let mut handed = event("AGENT_AUTHOR", "HANDED_TO_HUMAN", 2);
        handed.review_id = review_id;
        handed.notes = Some("Author's reasoning: the button is the only label".to_owned());
        let waiting = HumanRequiredReview {
            review_id,
            artifact_type: ReviewArtifact::ProductSchema,
            listing_source_name: Some("Antiques".to_owned()),
            agent_rounds: 1,
            repair_request: None,
            created: TO - Duration::hours(50),
        };

        let digest = Digest::of(
            FROM,
            TO,
            ReviewEventsSince {
                events: vec![handed],
                truncated: false,
            },
            vec![],
            ReviewQueue {
                workable: vec![],
                waiting_for_human: vec![waiting],
            },
            rolling_cost(),
        );

        let markdown = digest.markdown();
        assert_eq!(digest.handed_to_human.agents, 1);
        assert!(
            markdown.contains("| Handed to a human | 1 | 0 |"),
            "{markdown}"
        );
        assert!(
            markdown.contains(
                "1 review(s) wait for a human; the oldest opened at 2026-10-26T04:00:00Z (50 hours ago).\n"
            ),
            "{markdown}"
        );
        assert!(
            markdown.contains(&format!(
                "- {review_id} PRODUCT_SCHEMA · Antiques · 1 agent rounds · handed over by AGENT_AUTHOR at 2026-10-28T04:00:00Z: Author's reasoning: the button is the only label\n"
            )),
            "{markdown}"
        );
    }

    /// An agent's approval may rest on the author's record that it found no sold page instead of
    /// a tested one, so the digest counts the records and lists each with the author's account.
    #[test]
    fn should_count_and_list_the_records_that_no_sold_page_was_found() {
        let mut recorded = event("AGENT_AUTHOR", "NO_SOLD_PAGE_RECORDED", 2);
        recorded.notes = Some("Searched the sold archive and the filters".to_owned());
        let review_id = recorded.review_id;

        let digest = Digest::of(
            FROM,
            TO,
            ReviewEventsSince {
                events: vec![recorded, event("AGENT_AUTHOR", "PAGE_ADDED", 1)],
                truncated: false,
            },
            vec![],
            ReviewQueue::default(),
            rolling_cost(),
        );

        let markdown = digest.markdown();
        assert_eq!(
            digest.no_sold_page_recorded,
            DecisionCount {
                agents: 1,
                others: 0
            }
        );
        assert!(
            markdown.contains("| No sold page recorded | 1 | 0 |"),
            "{markdown}"
        );
        assert!(
            markdown.contains(&format!(
                "{review_id} PRODUCT_SCHEMA · Antiques · AGENT_AUTHOR NO_SOLD_PAGE_RECORDED at version 3: Searched the sold archive and the filters\n"
            )),
            "{markdown}"
        );
        assert!(!markdown.contains("PAGE_ADDED"), "{markdown}");
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
            created: TO - Duration::hours(2),
        };
        let none = Digest::of(
            FROM,
            TO,
            ReviewEventsSince {
                events: vec![],
                truncated: false,
            },
            vec![],
            ReviewQueue::default(),
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
            ReviewQueue {
                workable: vec![],
                waiting_for_human: vec![waiting],
            },
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
            ReviewQueue::default(),
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
