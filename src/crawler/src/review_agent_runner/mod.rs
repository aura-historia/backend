//! `review-agent-runner`: works the review console's pending reviews with the two review agents of
//! `agent` authoring mode, headless, through the Claude Code CLI that is logged in on the machine.
//! Per review it starts a fresh author `claude -p` process and then a fresh reviewer process, each
//! with only its role's `review-mcp` tools, and it records each process's report, tokens and cost.
//! It is an operator of the console like a human with a browser: it lists reviews and events with
//! the author agent's token, holds no database access and no review rule of its own, and the
//! console's role refusals and approval guard apply to everything the agents do. Everything
//! specific to Claude Code lives here, outside the crawler's services.
//!
//! One run is one pass over the pending reviews, under a lock so one runner works at a time. The
//! runner starts a run every interval until it is stopped, as a systemd service in production and
//! as a child of `dev` in development; `--once` starts a single run by hand. The contract is in
//! `docs/crawler/operations.md` (review agent runner).

mod child;
mod digest;
mod envelope;
mod invocation;
mod ledger;
mod login;
mod pass;
mod process;
mod queue;
mod reports;
mod settings;
mod state;

pub use child::{RunnerChild, runner_env, runner_executable_next_to};
pub use pass::{PassStop, PassSummary};
pub use settings::{
    AgentProcessSettings, RunMode, RunnerLocation, RunnerSettings, RunnerSettingsError,
    current_location,
};

use digest::{Digest, RollingCost};
use ledger::{FIVE_HOURS, LEDGER_RETENTION, ONE_WEEK, ProcessLedger};
use login::{LoginError, probe_login};
use pass::{PassError, usage_limit_name, work_pending_reviews};
use queue::{HumanRequiredReview, ReviewEventsSince, review_events_since, review_queue};
use state::{RunnerLock, RunnerState, StateError, load_state, save_state};

use crate::review_mcp::{
    AgentRole, ConsoleClient, ConsoleClientError, ConsoleError, RoleTokenFile,
};
use std::path::PathBuf;
use time::{Duration, OffsetDateTime};
use tracing::{error, info, warn};

/// The name `value` is serialized under, such as `LIMIT_REACHED`, for a log field or the digest;
/// empty for none.
fn wire_name<T: serde::Serialize>(value: Option<T>) -> String {
    value
        .and_then(|value| serde_json::to_value(value).ok())
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// How often the service's runs write a digest.
const DIGEST_INTERVAL: Duration = Duration::days(1);

/// What one run did.
#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    /// Another runner held the lock, so this run did nothing.
    Locked,
    Ran {
        summary: PassSummary,
        /// The digest this run wrote, if one was due.
        digest: Option<PathBuf>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("cannot create the runner's state directory {}", path.display())]
    StateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    State(#[from] StateError),
    #[error(transparent)]
    Ledger(#[from] ledger::LedgerError),
    #[error(
        "the runner refuses to start: the review agents' token file cannot be read, so the crawler is not in CRAWLER_AUTHORING_MODE=agent or CRAWLER_REVIEW_AGENT_TOKEN_FILE names another file"
    )]
    NotAgentMode(#[source] ConsoleClientError),
    #[error("the runner refuses to start: {0}")]
    Login(#[from] LoginError),
    #[error("the review agents' pass failed")]
    Pass(#[from] PassError),
}

/// The service: a run, then `settings.interval` of rest after it ends, until the process is
/// stopped. A run that fails is logged and the next one follows after the interval, so a console
/// that is not up yet or an expired login never ends the service; a state or ledger it cannot read
/// keeps failing each run, so nothing runs on a forgotten pause. The lock is held only
/// during a run, so a `--once` between two runs works.
pub async fn run_every_interval(settings: &RunnerSettings) {
    loop {
        if let Err(run_error) = run(settings).await {
            error!(
                error = %run_error,
                cause = ?std::error::Error::source(&run_error),
                interval_seconds = settings.interval.as_secs(),
                "Review agent run failed; the next run follows after the interval"
            );
        }
        tokio::time::sleep(settings.interval).await;
    }
}

/// One run: takes the lock, checks that the crawler is in `agent` mode and that the CLI is
/// logged in with a Claude subscription, works the pending reviews, and writes the digest when it
/// is due (the service) or right away (`--once`).
pub async fn run(settings: &RunnerSettings) -> Result<RunOutcome, RunnerError> {
    std::fs::create_dir_all(&settings.state_dir).map_err(|source| RunnerError::StateDirectory {
        path: settings.state_dir.clone(),
        source,
    })?;
    let Some(_lock) = RunnerLock::acquire(&settings.state_dir)? else {
        info!(
            state_dir = %settings.state_dir.display(),
            "Another review agent runner holds the lock; this start does nothing"
        );
        return Ok(RunOutcome::Locked);
    };
    let console = ConsoleClient::new(
        &settings.console.console_url,
        RoleTokenFile::new(settings.console.token_file.clone(), AgentRole::Author),
    )
    .map_err(RunnerError::NotAgentMode)?;
    let login = probe_login(&settings.claude_bin, &settings.child_env).await?;
    info!(
        mode = ?settings.mode,
        auth_method = %login.method,
        organization = login.organization.as_deref().unwrap_or(""),
        subscription = login.subscription.as_deref().unwrap_or(""),
        console_url = %settings.console.console_url,
        state_dir = %settings.state_dir.display(),
        "Review agent runner started with the CLI's Claude login"
    );
    let started = OffsetDateTime::now_utc();
    let mut state = load_state(&settings.state_dir)?;
    state.expire(started);
    let mut ledger = ProcessLedger::open(&settings.state_dir)?;
    ledger.prune(started - LEDGER_RETENTION)?;

    let (pass, mut state, ledger) = work_pending_reviews(settings, &console, state, ledger).await;
    save_state(&settings.state_dir, &state)?;
    let summary = pass?;
    let now = OffsetDateTime::now_utc();
    info!(
        pending = summary.pending,
        waiting_for_human = summary.waiting_for_human,
        handed_to_human = summary.handed_to_human,
        cooling_down = summary.cooling_down,
        worked = summary.worked,
        processes = summary.processes,
        decided = summary.decided,
        stopped = %PassStop::describe(summary.stopped),
        five_hour_cost_usd = ledger.cost_within(FIVE_HOURS, now),
        weekly_cost_usd = ledger.cost_within(ONE_WEEK, now),
        "Review agents pass finished"
    );
    let digest = write_digest(settings, &console, &mut state, &ledger, now).await;
    if digest.is_some() {
        save_state(&settings.state_dir, &state)?;
    }
    Ok(RunOutcome::Ran { summary, digest })
}

/// Writes the digest a run owes: the service's once a day has passed since the last one, a `--once`
/// run of the last day right away, with the reviews that wait for a human after this run's pass. A
/// digest that cannot be written is logged and owed again.
async fn write_digest(
    settings: &RunnerSettings,
    console: &ConsoleClient,
    state: &mut RunnerState,
    ledger: &ProcessLedger,
    now: OffsetDateTime,
) -> Option<PathBuf> {
    let from = match settings.mode {
        RunMode::Once => now - DIGEST_INTERVAL,
        RunMode::Loop => match state.last_digest {
            Some(last) if now - last < DIGEST_INTERVAL => return None,
            Some(last) => last.max(now - LEDGER_RETENTION),
            None => now - DIGEST_INTERVAL,
        },
    };
    let (events, waiting_for_human) = match digest_reads(console, from).await {
        Ok(read) => read,
        Err(error) => {
            warn!(
                error = %error,
                cause = ?std::error::Error::source(&error),
                "Cannot read the review events or queue for the digest; the next run writes it"
            );
            return None;
        }
    };
    let rolling = RollingCost {
        five_hour_cost_usd: ledger.cost_within(FIVE_HOURS, now),
        weekly_cost_usd: ledger.cost_within(ONE_WEEK, now),
        paused_until: state
            .pause_at(now)
            .map(|pause| (pause.until, usage_limit_name(pause.limit).to_owned())),
    };
    let digest = Digest::of(
        from,
        now,
        events,
        ledger.since(from).cloned().collect(),
        waiting_for_human,
        rolling,
    );
    match digest.write(&settings.state_dir) {
        Ok(path) => {
            info!(
                path = %path.display(),
                from = %digest::timestamp(digest.from),
                to = %digest::timestamp(digest.to),
                approved_by_agents = digest.approved.agents,
                approved_by_others = digest.approved.others,
                rejected_by_agents = digest.rejected.agents,
                rejected_by_others = digest.rejected.others,
                needs_repair_by_agents = digest.needs_repair.agents,
                needs_repair_by_others = digest.needs_repair.others,
                restored = digest.restored,
                needs_a_human = digest.needs_a_human.len(),
                refusals_by_failed_check = ?digest.refusals_by_check,
                refusals_by_kind = ?digest.refusals_by_kind,
                processes = ?digest.processes_by_end,
                input_tokens = digest.usage.input_tokens,
                output_tokens = digest.usage.output_tokens,
                cache_read_input_tokens = digest.usage.cache_read_input_tokens,
                cache_creation_input_tokens = digest.usage.cache_creation_input_tokens,
                total_cost_usd = digest.cost_usd,
                reviews = digest.reviews.len(),
                "Review agents digest"
            );
            if settings.mode == RunMode::Loop {
                state.last_digest = Some(now);
            }
            Some(path)
        }
        Err(error) => {
            warn!(error = %error, "Cannot write the review agents digest; the next run writes it");
            None
        }
    }
}

/// What the digest reads from the console: the events of every review since `from`, and the
/// pending reviews that wait for a human now.
async fn digest_reads(
    console: &ConsoleClient,
    from: OffsetDateTime,
) -> Result<(ReviewEventsSince, Vec<HumanRequiredReview>), ConsoleError> {
    Ok((
        review_events_since(console, from).await?,
        review_queue(console).await?.waiting_for_human,
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::CrawlerReviewId;
    use crate::review::model::CrawlerReview;
    use crate::review_agent_runner::ledger::LEDGER_FILE;
    use crate::review_agent_runner::ledger::tests::StateDirectory;
    use crate::review_agent_runner::settings::RunnerLocation;
    use listing_source_core::ListingSourceId;
    use serde_json::{Value, json};
    use std::collections::HashMap;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const AUTHOR_TOKEN: &str = "authortokenfortheconsole";
    const REVIEWER_TOKEN: &str = "reviewertokenfortheconsole";
    /// Variables of the runner's environment that no agent process may see.
    const DECOY_ENV: [(&str, &str); 6] = [
        ("ANTHROPIC_API_KEY", "decoy-api-key"),
        ("ANTHROPIC_BASE_URL", "https://decoy.example"),
        ("CLAUDE_CODE_USE_BEDROCK", "1"),
        ("AWS_SECRET_ACCESS_KEY", "decoy-aws-secret"),
        ("CRAWLER_REVIEW_AUTH_TOKEN", "decoy-human-token"),
        ("NODE_OPTIONS", "--require /tmp/decoy.js"),
    ];

    /// A review console that answers the review queue, a review and the event list to the
    /// author's token. The event list holds `events` and, as `AGENT_REVIEWER` events, the
    /// decisions the fake `claude` logged to its `decided.log`, as the review MCP server would
    /// have recorded them. A review the fake's author handed to a human (`human/<review id>`)
    /// waits for a human, and one marked `unreadable/<review id>` cannot be read.
    struct FakeConsole {
        url: String,
    }

    impl FakeConsole {
        async fn start(reviews: Vec<CrawlerReview>, events: Value, claude_dir: PathBuf) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .unwrap_or_else(|error| panic!("bind: {error}"));
            let url = format!(
                "http://{}",
                listener
                    .local_addr()
                    .unwrap_or_else(|error| panic!("{error}"))
            );
            let listed = Arc::new(serde_json::to_string(&reviews).unwrap_or_default());
            let reviews = Arc::new(reviews);
            let events = Arc::new(events);
            let claude_dir = Arc::new(claude_dir);
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    let mut request = Vec::new();
                    let mut buffer = [0_u8; 4096];
                    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                        match stream.read(&mut buffer).await {
                            Ok(0) | Err(_) => break,
                            Ok(read) => request.extend_from_slice(&buffer[..read]),
                        }
                    }
                    let request = String::from_utf8_lossy(&request).to_string();
                    let authorized = request
                        .lines()
                        .any(|line| line == format!("authorization: Bearer {AUTHOR_TOKEN}"));
                    let path = request.split_whitespace().nth(1).unwrap_or_default();
                    let (status, body) = if !authorized {
                        (401, "{\"error\":\"unauthorized\"}".to_owned())
                    } else if path == "/api/reviews" {
                        (200, listed.to_string())
                    } else if path.starts_with("/api/review-events?since=") {
                        let decided_log = claude_dir.join("decided.log");
                        (200, with_decisions(&events, &decided_log).to_string())
                    } else if let Some(review) = path.strip_prefix("/api/reviews/").and_then(|id| {
                        reviews
                            .iter()
                            .find(|review| review.review_id.to_string() == id)
                    }) {
                        review_detail(review, &claude_dir)
                    } else {
                        (404, "{\"error\":\"not found\"}".to_owned())
                    };
                    let response = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                }
            });
            Self { url }
        }
    }

    /// `GET /api/reviews/{id}` of `review`, as the fake `claude` in `claude_dir` left it.
    fn review_detail(review: &CrawlerReview, claude_dir: &Path) -> (u16, String) {
        let id = review.review_id.to_string();
        if claude_dir.join("unreadable").join(&id).exists() {
            return (500, "{\"error\":\"failed\"}".to_owned());
        }
        let mut review = review.clone();
        if claude_dir.join("human").join(&id).exists() {
            review.agent_rounds = 3;
            review.human_required = true;
        }
        let detail = json!({ "review": review, "pages": [], "urls": [], "events": [] });
        (200, detail.to_string())
    }

    /// `events` with an `AGENT_REVIEWER` event for each `<review id> <action>` line of `decided_log`.
    fn with_decisions(events: &Value, decided_log: &Path) -> Value {
        let mut events = events.clone();
        let decisions = std::fs::read_to_string(decided_log).unwrap_or_default();
        for (review_id, action) in decisions.lines().filter_map(|line| line.split_once(' ')) {
            if let Some(list) = events["events"].as_array_mut() {
                list.push(json!({
                    "event_id": crate::CrawlerReviewEventId::new().to_string(),
                    "review_id": review_id, "artifact_type": "PRODUCT_SCHEMA",
                    "listing_source_id": "ls_x", "listing_source_name": "Antiques",
                    "actor": "AGENT_REVIEWER", "action": action, "candidate_version": 2,
                    "notes": null,
                    "created": OffsetDateTime::now_utc()
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_default(),
                }));
            }
        }
        events
    }

    fn review(status: &str, crawl_enabled: bool) -> CrawlerReview {
        let now = OffsetDateTime::now_utc();
        CrawlerReview {
            review_id: CrawlerReviewId::new(),
            listing_source_id: ListingSourceId::new(),
            listing_source_name: Some("Antiques".to_owned()),
            listing_source_crawl_enabled: crawl_enabled,
            domain_id: None,
            artifact_type: "PRODUCT_SCHEMA".to_owned(),
            status: status.to_owned(),
            reason: "manual_schema_required".to_owned(),
            candidate_payload: json!({}),
            candidate_version: 1,
            validation_summary: json!({}),
            reviewer_notes: None,
            repair_request: None,
            agent_rounds: 0,
            human_required: false,
            created: now,
            updated: now,
            reviewed: None,
        }
    }

    /// A pending review the agents gave three needs-repair rounds, which waits for a human.
    fn waiting_for_human() -> CrawlerReview {
        CrawlerReview {
            agent_rounds: 3,
            human_required: true,
            repair_request: Some(crate::review::model::RepairRequest {
                note: "title still takes the shop name".to_owned(),
                requested: OffsetDateTime::now_utc(),
            }),
            ..review("PENDING_REVIEW", true)
        }
    }

    /// A `claude` executable that records each call's arguments, input and environment and
    /// answers the canned envelope of its role, or of its review and role. A reviewer process
    /// logs the decision of its review, or of the role, to `decided.log`, as the console records it.
    /// A process told to hang waits after its stream until it is stopped; one told to die kills
    /// itself. An author told to hand its review over gives the needs-repair verdict that reaches
    /// the cap, which the fake console then shows.
    struct FakeClaude {
        directory: StateDirectory,
    }

    const FAKE_CLAUDE: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
if [ "$1" = auth ]; then cat "$dir/auth.json"; exit 0; fi
role=reviewer
case "$*" in *'"--role","author"'*) role=author;; esac
prompt="$(cat)"
review="$(printf '%s' "$prompt" | grep -o 'cr_[0-9a-z]*' | head -n 1)"
call="$dir/calls/$(date +%s%N)-$$-$role"
mkdir -p "$call"
printf '%s\n' "$@" > "$call/argv"
printf '%s' "$prompt" > "$call/stdin"
env > "$call/env"
echo "start $role $review" >> "$dir/order.log"
if [ "$role" = author ] && [ -f "$dir/hand_over/$review" ]; then touch "$dir/human/$review"; fi
if [ "$role" = reviewer ]; then
  decision="$dir/decisions/$review"
  [ -f "$decision" ] || decision="$dir/decisions/reviewer"
  if [ -s "$decision" ]; then echo "$review $(cat "$decision")" >> "$dir/decided.log"; fi
fi
sleep 0.2
envelope="$dir/envelopes/$review-$role.json"
[ -f "$envelope" ] || envelope="$dir/envelopes/$role.json"
sed "s/__REVIEW_ID__/$review/g" "$envelope"
if [ -f "$dir/hang/$review-$role" ]; then sleep 30; fi
if [ -f "$dir/die/$review-$role" ]; then kill -KILL $$; fi
echo "end $role $review" >> "$dir/order.log"
"#;

    impl FakeClaude {
        fn new() -> Self {
            let directory = StateDirectory::new();
            let path = directory.path().join("claude");
            std::fs::write(&path, FAKE_CLAUDE).unwrap_or_default();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .unwrap_or_else(|error| panic!("{error}"));
            for subdirectory in [
                "envelopes",
                "decisions",
                "hang",
                "die",
                "hand_over",
                "human",
                "unreadable",
            ] {
                std::fs::create_dir(directory.path().join(subdirectory)).unwrap_or_default();
            }
            let fake = Self { directory };
            fake.login(json!({ "loggedIn": true, "authMethod": "claude.ai", "orgName": "Aura" }));
            fake.answer("author", author_envelope("STAGED", 0.5));
            fake.answer("reviewer", reviewer_envelope("APPROVE", "ACCEPTED", 0.25));
            fake.decide("reviewer", "APPROVED");
            fake
        }

        fn path(&self) -> PathBuf {
            self.directory.path().join("claude")
        }

        fn login(&self, status: Value) {
            std::fs::write(self.directory.path().join("auth.json"), status.to_string())
                .unwrap_or_default();
        }

        /// Answers `envelope` to the role, or to `<review id>-<role>`.
        fn answer(&self, name: &str, envelope: Value) {
            self.stream(name, &[envelope]);
        }

        /// Prints `messages` as the CLI's stream of JSON lines to the role, or to
        /// `<review id>-<role>`.
        fn stream(&self, name: &str, messages: &[Value]) {
            let lines: Vec<String> = messages.iter().map(Value::to_string).collect();
            std::fs::write(
                self.directory.path().join(format!("envelopes/{name}.json")),
                lines.join("\n"),
            )
            .unwrap_or_default();
        }

        /// Has the author of `review_id` give the needs-repair verdict that hands it to a human.
        fn hand_to_human(&self, review_id: CrawlerReviewId) {
            std::fs::write(
                self.directory
                    .path()
                    .join("hand_over")
                    .join(review_id.to_string()),
                "",
            )
            .unwrap_or_default();
        }

        /// Has the console fail to answer the review `review_id`.
        fn unreadable(&self, review_id: CrawlerReviewId) {
            std::fs::write(
                self.directory
                    .path()
                    .join("unreadable")
                    .join(review_id.to_string()),
                "",
            )
            .unwrap_or_default();
        }

        /// Has the process `<review id>-<role>` hang after its stream until it is stopped.
        fn hang(&self, name: &str) {
            std::fs::write(self.directory.path().join("hang").join(name), "").unwrap_or_default();
        }

        /// Has the process `<review id>-<role>` kill itself after its stream.
        fn die(&self, name: &str) {
            std::fs::write(self.directory.path().join("die").join(name), "").unwrap_or_default();
        }

        /// Has the reviewer of the review `name`, or every reviewer for `reviewer`, take `action`;
        /// an empty action takes none.
        fn decide(&self, name: &str, action: &str) {
            std::fs::write(self.directory.path().join("decisions").join(name), action)
                .unwrap_or_default();
        }

        fn order(&self) -> Vec<String> {
            std::fs::read_to_string(self.directory.path().join("order.log"))
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }

        /// Each call's recorded files, oldest first.
        fn calls(&self) -> Vec<HashMap<&'static str, String>> {
            let mut calls: Vec<PathBuf> = std::fs::read_dir(self.directory.path().join("calls"))
                .map(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .map(|entry| entry.path())
                        .collect()
                })
                .unwrap_or_default();
            calls.sort();
            calls
                .iter()
                .map(|call| {
                    ["argv", "stdin", "env"]
                        .into_iter()
                        .map(|file| {
                            (
                                file,
                                std::fs::read_to_string(call.join(file)).unwrap_or_default(),
                            )
                        })
                        .collect()
                })
                .collect()
        }
    }

    fn author_envelope(action: &str, cost: f64) -> Value {
        json!({
            "type": "result", "subtype": "success", "is_error": false, "num_turns": 9,
            "duration_ms": 1200, "total_cost_usd": cost,
            "usage": { "input_tokens": 100, "output_tokens": 50 },
            "modelUsage": { "claude-opus-5-5": {} },
            "result": "page text the runner must not log",
            "structured_output": {
                "review_id": "__REVIEW_ID__", "action": action, "candidate_version": 2,
                "summary": "Staged and tested on /item/1.", "risks": [],
            },
        })
    }

    fn reviewer_envelope(decision: &str, outcome: &str, cost: f64) -> Value {
        json!({
            "type": "result", "subtype": "success", "is_error": false, "num_turns": 5,
            "duration_ms": 800, "total_cost_usd": cost,
            "usage": { "input_tokens": 80, "output_tokens": 20 },
            "structured_output": {
                "review_id": "__REVIEW_ID__", "decision": decision, "confidence": "HIGH",
                "summary": "Checked /item/1.", "risks": [], "field_findings": [],
                "server_answer": {
                    "outcome": outcome,
                    "kind": if outcome == "REFUSED" { "CONFLICT" } else { "" },
                    "failed_checks": if outcome == "REFUSED" { json!(["LIVE_MATRIX_MISSING"]) } else { json!([]) },
                    "detail": "",
                },
            },
        })
    }

    struct Fixture {
        console: FakeConsole,
        claude: FakeClaude,
        state: StateDirectory,
        tokens: StateDirectory,
    }

    impl Fixture {
        async fn new(reviews: Vec<CrawlerReview>) -> Self {
            let events = json!({
                "events": [{
                    "event_id": crate::CrawlerReviewEventId::new().to_string(),
                    "review_id": CrawlerReviewId::new().to_string(),
                    "artifact_type": "URL_PATTERN", "listing_source_id": "ls_x",
                    "listing_source_name": "Antiques", "actor": "HUMAN", "action": "RESTORED",
                    "candidate_version": 4, "notes": "pattern took categories",
                    "created": (OffsetDateTime::now_utc() - Duration::hours(1))
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_default(),
                }],
                "truncated": false,
            });
            let tokens = StateDirectory::new();
            std::fs::write(
                tokens.path().join("review-agent-tokens.json"),
                json!({ "author": AUTHOR_TOKEN, "reviewer": REVIEWER_TOKEN }).to_string(),
            )
            .unwrap_or_default();
            let claude = FakeClaude::new();
            Self {
                console: FakeConsole::start(reviews, events, claude.directory.path().to_path_buf())
                    .await,
                claude,
                state: StateDirectory::new(),
                tokens,
            }
        }

        fn settings(&self, mode_args: &[&str], env: &[(&str, &str)]) -> RunnerSettings {
            let claude = self.claude.path();
            let state = self.state.path().display().to_string();
            let token_file = self.tokens.path().join("review-agent-tokens.json");
            let mut values: HashMap<String, String> = HashMap::from([
                (
                    "CRAWLER_REVIEW_CONSOLE_URL".to_owned(),
                    self.console.url.clone(),
                ),
                (
                    "CRAWLER_REVIEW_AGENT_TOKEN_FILE".to_owned(),
                    token_file.display().to_string(),
                ),
                (
                    "CRAWLER_REVIEW_AGENT_RUNNER_CLAUDE_BIN".to_owned(),
                    claude.display().to_string(),
                ),
                ("CRAWLER_REVIEW_AGENT_RUNNER_STATE_DIR".to_owned(), state),
                (
                    "CRAWLER_REVIEW_AGENT_RUNNER_MAX_PROCESSES".to_owned(),
                    "1".to_owned(),
                ),
                ("PATH".to_owned(), std::env::var("PATH").unwrap_or_default()),
                ("HOME".to_owned(), self.state.path().display().to_string()),
            ]);
            values.extend(
                DECOY_ENV
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
            );
            values.extend(
                env.iter()
                    .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
            );
            RunnerSettings::from_args_and_env(
                mode_args.iter().map(|arg| (*arg).to_owned()),
                |name| values.get(name).cloned(),
                &RunnerLocation {
                    working_dir: self.state.path().to_path_buf(),
                    executable: PathBuf::from("/opt/crawler/bin/review-agent-runner"),
                },
            )
            .unwrap_or_else(|error| panic!("{error}"))
        }

        fn ledger(&self) -> Vec<Value> {
            std::fs::read_to_string(self.state.path().join(LEDGER_FILE))
                .unwrap_or_default()
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect()
        }
    }

    async fn run_with(settings: &RunnerSettings) -> RunOutcome {
        run(settings)
            .await
            .unwrap_or_else(|error| panic!("{error}: {error:?}"))
    }

    fn summary(outcome: &RunOutcome) -> PassSummary {
        match outcome {
            RunOutcome::Ran { summary, .. } => summary.clone(),
            RunOutcome::Locked => panic!("the run found the lock held"),
        }
    }

    #[tokio::test]
    async fn should_work_each_pending_review_with_a_fresh_author_then_reviewer_process() {
        let first = review("PENDING_REVIEW", true);
        let second = review("PENDING_REVIEW", true);
        let fixture = Fixture::new(vec![
            first.clone(),
            review("PENDING_REVIEW", false),
            review("APPROVED", true),
            second.clone(),
        ])
        .await;

        let outcome = run_with(&fixture.settings(&["--once"], &[])).await;

        let summary = summary(&outcome);
        assert_eq!(
            (
                summary.pending,
                summary.worked,
                summary.processes,
                summary.decided
            ),
            (2, 2, 4, 2)
        );
        let (first_id, second_id) = (first.review_id, second.review_id);
        assert_eq!(
            fixture.claude.order(),
            [
                format!("start author {first_id}"),
                format!("end author {first_id}"),
                format!("start reviewer {first_id}"),
                format!("end reviewer {first_id}"),
                format!("start author {second_id}"),
                format!("end author {second_id}"),
                format!("start reviewer {second_id}"),
                format!("end reviewer {second_id}"),
            ]
        );
        let calls = fixture.claude.calls();
        assert_eq!(calls.len(), 4);
        for call in &calls {
            let argv: Vec<&str> = call["argv"].lines().collect();
            for flag in ["-p", "--no-session-persistence", "--strict-mcp-config"] {
                assert!(argv.contains(&flag), "{flag}: {argv:?}");
            }
            assert!(!argv.contains(&"--bare"));
            for token in [AUTHOR_TOKEN, REVIEWER_TOKEN] {
                for (file, text) in call {
                    assert!(!text.contains(token), "a token in the child's {file}");
                }
            }
            let mut names: Vec<&str> = call["env"]
                .lines()
                .filter_map(|line| line.split_once('=').map(|(name, _)| name))
                .filter(|name| !["PWD", "SHLVL", "_", "OLDPWD"].contains(name))
                .collect();
            names.sort_unstable();
            assert_eq!(
                names,
                [
                    "CLAUDE_CODE_DISABLE_AUTO_MEMORY",
                    "CLAUDE_CODE_DISABLE_CLAUDE_MDS",
                    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
                    "DISABLE_AUTOUPDATER",
                    "HOME",
                    "PATH",
                    "TZ",
                ],
                "only the allowlisted variables reach the child: {}",
                call["env"]
            );
            for (name, _) in DECOY_ENV {
                assert!(
                    !call["env"]
                        .lines()
                        .any(|line| line.starts_with(&format!("{name}="))),
                    "{name}: {}",
                    call["env"]
                );
            }
            assert!(!call["stdin"].is_empty());
        }
        let ledger = fixture.ledger();
        assert_eq!(ledger.len(), 4);
        assert_eq!(ledger[0]["role"], "author");
        assert_eq!(ledger[0]["prompt_version"], "author.v2");
        assert_eq!(ledger[1]["prompt_version"], "reviewer.v2");
        assert_eq!(ledger[0]["report"]["action"], "STAGED");
        assert_eq!(ledger[1]["report"]["decision"], "APPROVE");
        assert_eq!(ledger[1]["total_cost_usd"], 0.25);
        assert!(
            !std::fs::read_to_string(fixture.state.path().join(LEDGER_FILE))
                .unwrap_or_default()
                .contains("page text the runner must not log")
        );
        let state = load_state(fixture.state.path()).unwrap_or_else(|e| panic!("{e}"));
        assert!(state.cooldowns.is_empty(), "{state:?}");
        let RunOutcome::Ran {
            digest: Some(digest),
            ..
        } = outcome
        else {
            panic!("--once wrote no digest");
        };
        let digest = std::fs::read_to_string(digest).unwrap_or_default();
        assert!(digest.contains("| Restored | – | 1 |"), "{digest}");
        assert!(digest.contains(&first_id.to_string()), "{digest}");
        assert!(digest.contains("4 processes (4 reported)"), "{digest}");
    }

    /// A review that waits for a human is no agent work: no process starts for it, it does not cool
    /// down, and the digest lists it with its last needs-repair note.
    #[tokio::test]
    async fn should_skip_a_review_that_waits_for_a_human_and_list_it_in_the_digest() {
        let waiting = waiting_for_human();
        let workable = review("PENDING_REVIEW", true);
        let fixture = Fixture::new(vec![waiting.clone(), workable.clone()]).await;

        let outcome = run_with(&fixture.settings(&["--once"], &[])).await;

        let summary = summary(&outcome);
        assert_eq!(
            (
                summary.pending,
                summary.waiting_for_human,
                summary.worked,
                summary.processes,
                summary.cooling_down
            ),
            (1, 1, 1, 2, 0)
        );
        assert!(
            fixture
                .claude
                .order()
                .iter()
                .all(|line| line.ends_with(&workable.review_id.to_string())),
            "{:?}",
            fixture.claude.order()
        );
        let state = load_state(fixture.state.path()).unwrap_or_else(|e| panic!("{e}"));
        assert!(state.cooldowns.is_empty(), "{state:?}");
        let RunOutcome::Ran {
            digest: Some(digest),
            ..
        } = outcome
        else {
            panic!("--once wrote no digest");
        };
        let digest = std::fs::read_to_string(digest).unwrap_or_default();
        assert!(
            digest.contains(&format!(
                "- {} PRODUCT_SCHEMA · Antiques · 3 agent rounds · last needs-repair at",
                waiting.review_id
            )),
            "{digest}"
        );
        assert!(
            digest.contains(": title still takes the shop name"),
            "{digest}"
        );
    }

    /// The author's own needs-repair verdict that reaches the cap hands the review to a human: the
    /// runner starts no reviewer process, whose every verdict the console would refuse with
    /// `HUMAN_REQUIRED`, and gives the review no cooldown. A review the console cannot answer
    /// gets its reviewer as before.
    #[tokio::test]
    async fn should_start_no_reviewer_for_a_review_that_waits_for_a_human_after_the_author() {
        let handed_over = review("PENDING_REVIEW", true);
        let unreadable = review("PENDING_REVIEW", true);
        let fixture = Fixture::new(vec![handed_over.clone(), unreadable.clone()]).await;
        fixture.claude.hand_to_human(handed_over.review_id);
        fixture.claude.unreadable(unreadable.review_id);

        let run = summary(&run_with(&fixture.settings(&[], &[])).await);

        assert_eq!(
            (run.worked, run.processes, run.handed_to_human, run.decided),
            (2, 3, 1, 1)
        );
        let (handed_over_id, unreadable_id) = (handed_over.review_id, unreadable.review_id);
        assert_eq!(
            fixture.claude.order(),
            [
                format!("start author {handed_over_id}"),
                format!("end author {handed_over_id}"),
                format!("start author {unreadable_id}"),
                format!("end author {unreadable_id}"),
                format!("start reviewer {unreadable_id}"),
                format!("end reviewer {unreadable_id}"),
            ]
        );
        let state = load_state(fixture.state.path()).unwrap_or_else(|e| panic!("{e}"));
        assert!(state.cooldowns.is_empty(), "{state:?}");
    }

    #[tokio::test]
    async fn should_cool_down_a_review_whose_author_ends_without_a_decision() {
        let limited = review("PENDING_REVIEW", true);
        let other = review("PENDING_REVIEW", true);
        let fixture = Fixture::new(vec![limited.clone(), other.clone()]).await;
        fixture.claude.answer(
            &format!("{}-author", limited.review_id),
            json!({
                "type": "result", "subtype": "error_max_turns", "is_error": true,
                "terminal_reason": "max_turns", "total_cost_usd": 0.75, "num_turns": 80,
            }),
        );
        let settings = fixture.settings(&[], &[]);

        let first_run = summary(&run_with(&settings).await);
        let second_run = summary(&run_with(&settings).await);

        assert_eq!((first_run.worked, first_run.processes), (2, 3));
        assert_eq!(first_run.decided, 1);
        let ledger = fixture.ledger();
        assert_eq!(ledger[0]["end"], "LIMIT_REACHED");
        assert_eq!(ledger[0]["limit"], "TURNS");
        assert_eq!(ledger[0]["subtype"], "error_max_turns");
        assert_eq!(ledger[0]["terminal_reason"], "max_turns");
        let state = load_state(fixture.state.path()).unwrap_or_else(|e| panic!("{e}"));
        assert!(state.cooldowns.contains_key(&limited.review_id));
        assert!(!state.cooldowns.contains_key(&other.review_id));
        assert_eq!(
            (second_run.cooling_down, second_run.worked),
            (1, 1),
            "the second run works only the review that is not cooling down"
        );
        assert!(
            fixture
                .claude
                .order()
                .iter()
                .skip(6)
                .all(|line| line.ends_with(&other.review_id.to_string()))
        );
    }

    /// The CLI's stream reports the limit with a `rate_limit_event` before its error result: the
    /// event's `resetsAt` is the reset time, whatever the message's wording says.
    #[tokio::test]
    async fn should_pause_on_a_usage_limit_and_start_nothing_until_it_resets() {
        let limited = review("PENDING_REVIEW", true);
        let fixture = Fixture::new(vec![limited.clone(), review("PENDING_REVIEW", true)]).await;
        let reset = (OffsetDateTime::now_utc() + Duration::days(2))
            .replace_time(time::Time::from_hms(9, 0, 0).unwrap_or(time::Time::MIDNIGHT));
        fixture.claude.stream(
            "author",
            &[
                json!({ "type": "system", "subtype": "init", "model": "claude-opus-5-5" }),
                json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "page text the runner must not log" }] } }),
                json!({
                    "type": "rate_limit_event",
                    "rate_limit_info": { "status": "rejected", "resetsAt": reset.unix_timestamp() },
                }),
                json!({
                    "type": "result", "subtype": "success", "is_error": true,
                    "api_error_status": 429, "total_cost_usd": 0.1,
                    "result": "You've hit your weekly limit · resets in a wording the runner cannot read",
                }),
            ],
        );
        let settings = fixture.settings(&[], &[]);

        let first_run = summary(&run_with(&settings).await);
        let second_run = summary(&run_with(&settings).await);

        assert_eq!(first_run.processes, 1);
        let state = load_state(fixture.state.path()).unwrap_or_else(|e| panic!("{e}"));
        let pause = state
            .paused
            .unwrap_or_else(|| panic!("no pause: {state:?}"));
        assert_eq!(pause.limit, envelope::UsageLimitKind::Weekly);
        assert!(pause.reported_reset);
        assert_eq!(pause.until, reset);
        assert!(matches!(first_run.stopped, Some(PassStop::Paused(_))));
        assert!(
            state.cooldowns.is_empty(),
            "a paused review does not cool down"
        );
        assert_eq!(second_run.processes, 0);
        assert_eq!(second_run.stopped, Some(PassStop::Paused(pause.until)));
        assert_eq!(fixture.ledger()[0]["usage_limit"], "WEEKLY");
    }

    /// A process stopped at its timeout, or killed, before its result envelope reported no cost:
    /// the runner charges it the highest cost a process reported in the ledger, which the rolling
    /// sums count.
    #[tokio::test]
    async fn should_charge_a_stopped_or_killed_process_the_highest_reported_cost() {
        let worked = review("PENDING_REVIEW", true);
        let stopped = review("PENDING_REVIEW", true);
        let killed = review("PENDING_REVIEW", true);
        let fixture = Fixture::new(vec![worked.clone(), stopped.clone(), killed.clone()]).await;
        for unfinished in [&stopped, &killed] {
            fixture.claude.stream(
                &format!("{}-author", unfinished.review_id),
                &[
                    json!({ "type": "system", "subtype": "init", "model": "claude-opus-5-5" }),
                    json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "page text" }] } }),
                ],
            );
        }
        fixture
            .claude
            .hang(&format!("{}-author", stopped.review_id));
        fixture.claude.die(&format!("{}-author", killed.review_id));
        let mut settings = fixture.settings(&[], &[]);
        settings.process_timeout = std::time::Duration::from_secs(2);

        let run = summary(&run_with(&settings).await);

        assert_eq!((run.worked, run.processes, run.decided), (3, 4, 1));
        let ledger = fixture.ledger();
        let costs: Vec<(Value, Value, Value)> = ledger
            .iter()
            .map(|record| {
                (
                    record["review_id"].clone(),
                    record["total_cost_usd"].clone(),
                    record.get("cost_charged").cloned().unwrap_or(json!(false)),
                )
            })
            .collect();
        assert_eq!(
            costs,
            [
                (
                    json!(worked.review_id.to_string()),
                    json!(0.5),
                    json!(false)
                ),
                (
                    json!(worked.review_id.to_string()),
                    json!(0.25),
                    json!(false)
                ),
                (
                    json!(stopped.review_id.to_string()),
                    json!(0.5),
                    json!(true)
                ),
                (json!(killed.review_id.to_string()), json!(0.5), json!(true)),
            ]
        );
        assert!(
            ledger[2]["failure"]
                .as_str()
                .is_some_and(|failure| failure.starts_with("stopped after its timeout")),
            "{}",
            ledger[2]
        );
        assert_eq!(ledger[3]["failure"], "no result envelope, exit code None");
        let state = load_state(fixture.state.path()).unwrap_or_else(|e| panic!("{e}"));
        assert!(state.cooldowns.contains_key(&stopped.review_id));
        assert!(state.cooldowns.contains_key(&killed.review_id));
    }

    /// The event trail, not the reviewer's report, says whether the console took a decision: a
    /// reviewer that reports its refused approval although its needs-repair verdict was accepted
    /// decided, and one that reports an acceptance the trail lacks did not.
    #[tokio::test]
    async fn should_judge_the_reviewer_s_decision_by_the_event_trail_not_its_report() {
        let repaired = review("PENDING_REVIEW", true);
        let unrecorded = review("PENDING_REVIEW", true);
        let fixture = Fixture::new(vec![repaired.clone(), unrecorded.clone()]).await;
        fixture.claude.answer(
            &format!("{}-reviewer", repaired.review_id),
            reviewer_envelope("NEEDS_REPAIR", "REFUSED", 0.25),
        );
        fixture
            .claude
            .decide(&repaired.review_id.to_string(), "REPAIR_REQUESTED");
        fixture.claude.decide(&unrecorded.review_id.to_string(), "");

        let run = summary(&run_with(&fixture.settings(&[], &[])).await);

        assert_eq!((run.processes, run.decided), (4, 1));
        let state = load_state(fixture.state.path()).unwrap_or_else(|e| panic!("{e}"));
        assert!(!state.cooldowns.contains_key(&repaired.review_id));
        assert!(state.cooldowns.contains_key(&unrecorded.review_id));
    }

    /// The agents run on the login's subscription: only its usage limits stop a pass, never the
    /// dollar cost the processes report.
    #[tokio::test]
    async fn should_keep_starting_processes_whatever_dollar_cost_they_report() {
        let fixture = Fixture::new(vec![
            review("PENDING_REVIEW", true),
            review("PENDING_REVIEW", true),
        ])
        .await;
        fixture
            .claude
            .answer("author", author_envelope("STAGED", 500.0));
        fixture
            .claude
            .answer("reviewer", reviewer_envelope("APPROVE", "ACCEPTED", 500.0));

        let run = summary(&run_with(&fixture.settings(&[], &[])).await);

        assert_eq!((run.worked, run.processes, run.stopped), (2, 4, None));
    }

    #[tokio::test]
    async fn should_bound_how_many_processes_run_at_once() {
        let fixture = Fixture::new((0..4).map(|_| review("PENDING_REVIEW", true)).collect()).await;
        let settings = fixture.settings(&[], &[("CRAWLER_REVIEW_AGENT_RUNNER_MAX_PROCESSES", "2")]);

        let run = summary(&run_with(&settings).await);

        assert_eq!(run.processes, 8);
        let mut running = 0_i32;
        let mut most = 0;
        for line in fixture.claude.order() {
            running += if line.starts_with("start") { 1 } else { -1 };
            most = most.max(running);
        }
        assert!((1..=2).contains(&most), "{most}");
    }

    #[tokio::test]
    async fn should_refuse_to_run_without_the_token_file_or_a_subscription_login() {
        let fixture = Fixture::new(vec![review("PENDING_REVIEW", true)]).await;
        let settings = fixture.settings(&[], &[]);
        fixture
            .claude
            .login(json!({ "loggedIn": false, "authMethod": "none" }));

        let logged_out = run(&settings).await;
        std::fs::remove_file(&settings.console.token_file).unwrap_or_default();
        let no_token_file = run(&settings).await;

        assert!(matches!(
            logged_out,
            Err(RunnerError::Login(LoginError::NotLoggedIn))
        ));
        assert!(matches!(no_token_file, Err(RunnerError::NotAgentMode(_))));
        assert!(fixture.claude.calls().is_empty());
    }

    #[tokio::test]
    async fn should_do_nothing_while_another_runner_holds_the_lock() {
        let fixture = Fixture::new(vec![review("PENDING_REVIEW", true)]).await;
        let settings = fixture.settings(&[], &[]);
        let _held = RunnerLock::acquire(fixture.state.path())
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("lock taken"));

        let outcome = run_with(&settings).await;

        assert_eq!(outcome, RunOutcome::Locked);
        assert!(fixture.claude.calls().is_empty());
        assert!(!Path::new(&fixture.state.path().join(LEDGER_FILE)).exists());
    }

    #[tokio::test]
    async fn should_run_again_after_the_interval_when_a_run_fails() {
        let fixture = Fixture::new(vec![]).await;
        let settings = fixture.settings(
            &[],
            &[("CRAWLER_REVIEW_AGENT_RUNNER_INTERVAL_SECONDS", "1")],
        );
        let token_file = settings.console.token_file.clone();
        let moved_away = token_file.with_extension("moved");
        std::fs::rename(&token_file, &moved_away).unwrap_or_else(|e| panic!("{e}"));

        let (service, ()) = tokio::join!(
            tokio::time::timeout(
                std::time::Duration::from_millis(2500),
                run_every_interval(&settings)
            ),
            async {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                std::fs::rename(&moved_away, &token_file).unwrap_or_else(|e| panic!("{e}"));
            }
        );

        assert!(service.is_err(), "the service ended");
        let state = load_state(fixture.state.path()).unwrap_or_else(|e| panic!("{e}"));
        assert!(
            state.last_digest.is_some(),
            "the run after the failed one worked and wrote the daily digest"
        );
    }

    #[tokio::test]
    async fn should_write_the_service_s_digest_once_a_day() {
        let fixture = Fixture::new(vec![]).await;
        let settings = fixture.settings(&[], &[]);

        let first = run_with(&settings).await;
        let second = run_with(&settings).await;

        assert!(matches!(
            first,
            RunOutcome::Ran {
                digest: Some(_),
                ..
            }
        ));
        assert!(matches!(second, RunOutcome::Ran { digest: None, .. }));
        let state = load_state(fixture.state.path()).unwrap_or_else(|e| panic!("{e}"));
        assert!(state.last_digest.is_some());
    }
}
