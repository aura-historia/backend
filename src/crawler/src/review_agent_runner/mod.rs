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
//! as a child of `dev` in development; `--once` starts a single run by hand. Stopped, it stops its
//! running agent processes and records their cost before it exits. The contract is in
//! `docs/crawler/operations.md` (review agent runner).

mod child;
mod decision_cap;
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
mod stop;

pub use child::{RunnerChild, runner_env, runner_executable_next_to};
pub use pass::{PassStop, PassSummary};
pub use settings::{
    AgentProcessSettings, RunMode, RunnerLocation, RunnerSettings, RunnerSettingsError,
    current_location,
};
pub use stop::{AGENT_DRAIN_DEADLINE, RunnerStop, STOP_DEADLINE};

use digest::{Digest, RollingCost};
use ledger::{FIVE_HOURS, LEDGER_RETENTION, ONE_WEEK, ProcessLedger};
use login::{LoginError, probe_login};
use pass::{PassError, usage_limit_name, work_pending_reviews};
use queue::{HumanRequiredReview, ReviewEventsSince, review_events_since, review_queue};
use state::{RunnerLock, RunnerState, StateError, load_state, save_state};

use crate::review::agent_decisions::{AGENT_LIMITS_ROUTE, AgentLimits};
use crate::review_mcp::{
    AgentRole, ConsoleClient, ConsoleClientError, ConsoleError, RoleTokenFile,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use time::{Duration, OffsetDateTime};
use tracing::{debug, error, info, warn};

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
    #[error(
        "cannot read the limits of the agents' decisions from the review console's {AGENT_LIMITS_ROUTE}; a console of an older build does not answer them, so deploy the runner and review-mcp from the crawler server's build"
    )]
    AgentLimits(#[source] ConsoleError),
    #[error("the review agents' pass failed")]
    Pass(#[from] PassError),
}

/// The service: a run, then `settings.interval` of rest after it ends, until `stop` is requested.
/// A run that fails is logged and the next one follows after the interval, so a console that is
/// not up yet or an expired login never ends the service; a state or ledger it cannot read keeps
/// failing each run, so nothing runs on a forgotten pause. The lock is held only during a run, so
/// a `--once` between two runs works.
pub async fn run_every_interval(settings: &RunnerSettings, stop: &RunnerStop) {
    while !stop.is_requested() {
        match run(settings, stop).await {
            Ok(_) => {}
            Err(run_error) if stop.is_requested() => error!(
                error = %run_error,
                cause = ?std::error::Error::source(&run_error),
                "Review agent run failed while the runner stops"
            ),
            Err(run_error) => error!(
                error = %run_error,
                cause = ?std::error::Error::source(&run_error),
                interval_seconds = settings.interval.as_secs(),
                "Review agent run failed; the next run follows after the interval"
            ),
        }
        tokio::select! {
            () = tokio::time::sleep(settings.interval) => {}
            () = stop.requested() => {}
        }
    }
}

/// One run: takes the lock, checks that the crawler is in `agent` mode and that the CLI is
/// logged in with a Claude subscription, reads the limits of the agents' decisions from the
/// console, works the pending reviews until `stop` is requested, and writes the digest when it is
/// due (the service) or right away (`--once`). A run that `stop` ended writes no digest; the next
/// run of the service writes it.
pub async fn run(settings: &RunnerSettings, stop: &RunnerStop) -> Result<RunOutcome, RunnerError> {
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
    let limits = agent_limits(&console)
        .await
        .map_err(RunnerError::AgentLimits)?;
    log_login(&LOGIN_ANNOUNCED, settings, &login, &limits);
    let started = OffsetDateTime::now_utc();
    let mut state = load_state(&settings.state_dir)?;
    if let Some(ended) = state.expire(started) {
        decision_cap::log_reviewer_pause_ended(ended);
    }
    let mut ledger = ProcessLedger::open(&settings.state_dir)?;
    ledger.prune(started - LEDGER_RETENTION)?;

    let (pass, mut state, ledger) =
        work_pending_reviews(settings, &console, limits, stop, state, ledger).await;
    save_state(&settings.state_dir, &state)?;
    let summary = pass?;
    let now = OffsetDateTime::now_utc();
    info!(
        pending = summary.pending,
        waiting_for_human = summary.waiting_for_human,
        handed_to_human = summary.handed_to_human,
        reviewers_held_back = summary.reviewers_held_back,
        cooling_down = summary.cooling_down,
        worked = summary.worked,
        processes = summary.processes,
        decided = summary.decided,
        stopped = %PassStop::describe(summary.stopped),
        five_hour_cost_usd = ledger.cost_within(FIVE_HOURS, now),
        weekly_cost_usd = ledger.cost_within(ONE_WEEK, now),
        "Review agents pass finished"
    );
    if stop.is_requested() {
        return Ok(RunOutcome::Ran {
            summary,
            digest: None,
        });
    }
    let digest = write_digest(settings, &console, &mut state, &ledger, now).await;
    if digest.is_some() {
        save_state(&settings.state_dir, &state)?;
    }
    Ok(RunOutcome::Ran { summary, digest })
}

/// Whether this process has told its login yet.
static LOGIN_ANNOUNCED: AtomicBool = AtomicBool::new(false);

/// Tells the CLI's login the run starts with: at info once per process start, at debug on every
/// later run, which probes the login again so that a logout fails the run closed. `announced`
/// is the process's one memory of having told it.
fn log_login(
    announced: &AtomicBool,
    settings: &RunnerSettings,
    login: &login::CliLogin,
    limits: &AgentLimits,
) {
    macro_rules! announce {
        ($log:ident) => {
            $log!(
                mode = ?settings.mode,
                auth_method = %login.method,
                organization = login.organization.as_deref().unwrap_or(""),
                subscription = login.subscription.as_deref().unwrap_or(""),
                console_url = %settings.console.console_url,
                state_dir = %settings.state_dir.display(),
                agent_min_labelled_urls = limits.min_labelled_urls.get(),
                "Review agent runner started with the CLI's Claude login"
            )
        };
    }
    match announced.swap(true, Ordering::Relaxed) {
        false => announce!(info),
        true => announce!(debug),
    }
}

/// The limits of the agents' decisions the console applies, which the prompts tell the agents:
/// the runner keeps no copy of the console's settings that could disagree with them.
async fn agent_limits(console: &ConsoleClient) -> Result<AgentLimits, ConsoleError> {
    console.get(AGENT_LIMITS_ROUTE).await?.decode()
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
        reviewers_paused_until: state.reviewer_pause_at(now).map(|pause| pause.until),
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
                handed_to_human_by_agents = digest.handed_to_human.agents,
                no_sold_page_recorded_by_agents = digest.no_sold_page_recorded.agents,
                no_sold_page_recorded_by_others = digest.no_sold_page_recorded.others,
                restored = digest.restored,
                revoked_rejections = digest.revoked_rejections,
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
    use crate::review::model::{
        CrawlerReview, PendingReviewPage, PendingReviewSummary, PendingReviewTotals,
    };
    use crate::review_agent_runner::ledger::LEDGER_FILE;
    use crate::review_agent_runner::ledger::tests::{StateDirectory, write_executable};
    use crate::review_agent_runner::settings::RunnerLocation;
    use crate::review_agent_runner::state::ReviewerPause;
    use listing_source_core::ListingSourceId;
    use serde_json::{Value, json};
    use std::collections::HashMap;
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

    /// The label minimum the fake console applies, which no runner setting names.
    const CONSOLE_MIN_LABELLED_URLS: u32 = 5;

    /// A review console that answers the review queue, a review, the event list and its agent
    /// limits to the author's token. The event list holds `events` and, as `AGENT_REVIEWER` events,
    /// the decisions the fake `claude` logged to its `decided.log`, as the review MCP server would
    /// have recorded them. A review a process of the fake handed to a human (`human/<review id>`)
    /// waits for a human, one a human handed back lists the time of it (`handed_back/<review
    /// id>`) in the queue, one marked `unreadable/<review id>` cannot be read, and the limits cannot
    /// be read once `limits_unreadable` exists.
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
            let reviews = Arc::new(reviews);
            let events = Arc::new(events);
            let claude_dir = Arc::new(claude_dir);
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    // Each connection on its own task, so a client that connects and sends
                    // nothing yet, such as a pooled connection, holds no other request back.
                    let (reviews, events, claude_dir) = (
                        Arc::clone(&reviews),
                        Arc::clone(&events),
                        Arc::clone(&claude_dir),
                    );
                    tokio::spawn(async move {
                        let mut request = Vec::new();
                        let mut buffer = [0_u8; 4096];
                        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                            match stream.read(&mut buffer).await {
                                Ok(0) | Err(_) => break,
                                Ok(read) => request.extend_from_slice(&buffer[..read]),
                            }
                        }
                        let content_length = String::from_utf8_lossy(&request)
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .and_then(|length| length.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        let header_end = request
                            .windows(4)
                            .position(|window| window == b"\r\n\r\n")
                            .map_or(request.len(), |at| at + 4);
                        while request.len() < header_end + content_length {
                            match stream.read(&mut buffer).await {
                                Ok(0) | Err(_) => break,
                                Ok(read) => request.extend_from_slice(&buffer[..read]),
                            }
                        }
                        let request = String::from_utf8_lossy(&request).to_string();
                        let body = request.get(header_end..).unwrap_or_default().to_owned();
                        let authorized = request
                            .lines()
                            .any(|line| line == format!("authorization: Bearer {AUTHOR_TOKEN}"));
                        let path = request.split_whitespace().nth(1).unwrap_or_default();
                        let (status, body) = if !authorized {
                            (401, "{\"error\":\"unauthorized\"}".to_owned())
                        } else if let Some(query) = path.strip_prefix("/api/pending-reviews?") {
                            (200, pending_page(&reviews, query, &claude_dir))
                        } else if path.starts_with("/api/review-events?since=") {
                            let decided_log = claude_dir.join("decided.log");
                            (200, with_decisions(&events, &decided_log).to_string())
                        } else if let Some(review) = path
                            .strip_prefix("/api/reviews/")
                            .and_then(|rest| rest.strip_suffix("/hand-to-human"))
                            .filter(|_| request.starts_with("POST "))
                        {
                            hand_over(review, &body, &claude_dir)
                        } else if let Some(review) =
                            path.strip_prefix("/api/reviews/").and_then(|id| {
                                reviews
                                    .iter()
                                    .find(|review| review.review_id.to_string() == id)
                            })
                        {
                            review_detail(review, &claude_dir)
                        } else if path == AGENT_LIMITS_ROUTE
                            && !claude_dir.join("limits_unreadable").exists()
                        {
                            let limits = AgentLimits {
                                min_labelled_urls: std::num::NonZeroU32::new(
                                    CONSOLE_MIN_LABELLED_URLS,
                                )
                                .unwrap_or(std::num::NonZeroU32::MIN),
                                ..AgentLimits::default()
                            };
                            (200, json!(limits).to_string())
                        } else {
                            (404, "{\"error\":\"not found\"}".to_owned())
                        };
                        let response = format!(
                            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.shutdown().await;
                    });
                }
            });
            Self { url }
        }
    }

    /// `GET /api/pending-reviews?<query>` of `reviews`, one review a page, so the runner reads the
    /// agents' queue page by page as it must however long the queue is.
    fn pending_page(reviews: &[CrawlerReview], query: &str, claude_dir: &Path) -> String {
        let query: HashMap<String, String> = url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
        let queue: Vec<&CrawlerReview> = reviews
            .iter()
            .filter(|review| {
                review.status == "PENDING_REVIEW" && review.listing_source_crawl_enabled
            })
            .filter(|review| {
                let late = claude_dir
                    .join("late")
                    .join(review.review_id.to_string())
                    .exists();
                let run_started = std::fs::read_to_string(claude_dir.join("order.log"))
                    .is_ok_and(|order| order.contains("start author"));
                !late || run_started
            })
            .collect();
        let waits = |review: &CrawlerReview| {
            review.human_required
                || claude_dir
                    .join("human")
                    .join(review.review_id.to_string())
                    .exists()
        };
        let count = |human_required: bool| {
            i64::try_from(
                queue
                    .iter()
                    .filter(|review| waits(review) == human_required)
                    .count(),
            )
            .unwrap_or(i64::MAX)
        };
        let totals = PendingReviewTotals {
            workable: count(false),
            waiting_for_human: count(true),
        };
        let selected: Vec<&CrawlerReview> = queue
            .into_iter()
            .filter(|review| {
                query
                    .get("human_required")
                    .is_none_or(|wanted| waits(review).to_string() == *wanted)
            })
            .collect();
        let start = query.get("after").map_or(0, |after| {
            selected
                .iter()
                .position(|review| review.review_id.to_string() == *after)
                .map_or(selected.len(), |index| index + 1)
        });
        let page: Vec<PendingReviewSummary> = selected
            .iter()
            .skip(start)
            .take(1)
            .map(|review| PendingReviewSummary {
                review_id: review.review_id,
                listing_source_id: review.listing_source_id,
                listing_source_name: review.listing_source_name.clone().unwrap_or_default(),
                artifact_type: review.artifact_type.clone(),
                reason: review.reason.clone(),
                candidate_version: review.candidate_version,
                repair_request: review.repair_request.clone(),
                agent_rounds: review.agent_rounds,
                human_required: waits(review),
                handed_back: std::fs::read_to_string(
                    claude_dir
                        .join("handed_back")
                        .join(review.review_id.to_string()),
                )
                .ok()
                .and_then(|handed_back| {
                    OffsetDateTime::parse(
                        &handed_back,
                        &time::format_description::well_known::Rfc3339,
                    )
                    .ok()
                }),
                created: review.created,
                updated: review.updated,
            })
            .collect();
        let next_after = (start + page.len() < selected.len())
            .then(|| page.last().map(|review| review.review_id))
            .flatten();
        serde_json::to_string(&PendingReviewPage {
            reviews: page,
            next_after,
            totals,
        })
        .unwrap_or_default()
    }

    /// `POST /api/reviews/{id}/hand-to-human` of the review `review_id`: logs `<review id>
    /// <notes>` to `handovers.log` and, as the console does, leaves the review waiting for a human.
    /// A review marked `handover_refused/<review id>` answers `409`.
    fn hand_over(review_id: &str, body: &str, claude_dir: &Path) -> (u16, String) {
        if claude_dir.join("handover_refused").join(review_id).exists() {
            return (409, "{\"error\":\"refused\"}".to_owned());
        }
        let notes = serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|body| body["notes"].as_str().map(str::to_owned))
            .unwrap_or_default();
        let mut log = std::fs::read_to_string(claude_dir.join("handovers.log")).unwrap_or_default();
        log.push_str(&format!("{review_id} {notes}\n"));
        let _ = std::fs::write(claude_dir.join("handovers.log"), log);
        let _ = std::fs::write(claude_dir.join("human").join(review_id), "");
        (200, "{\"ok\":true}".to_owned())
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
        let judged = claude_dir.join("judged").join(&id).exists();
        let events: Vec<Value> = judged
            .then(|| {
                json!({
                    "actor": "AGENT_REVIEWER", "action": "REPAIR_REQUESTED",
                    "candidate_version": review.candidate_version,
                    "created": OffsetDateTime::now_utc()
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_default(),
                })
            })
            .into_iter()
            .collect();
        let detail = json!({ "review": review, "pages": [], "urls": [], "events": events });
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
            listing_source_crawl_host: Some("antiques.example".to_owned()),
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
    /// A process told to hang waits after its stream until it is stopped: on SIGINT it logs that it
    /// was interrupted and prints the result envelope it was given for that, if any, as the CLI
    /// ends its turn. One told to die kills itself. One told to hand its review over gives the
    /// needs-repair verdict that reaches the cap, which the fake console then shows until a human
    /// hands the review back.
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
if [ -f "$dir/hand_over/$review-$role" ]; then
  target="$(cat "$dir/hand_over/$review-$role")"
  touch "$dir/human/${target:-$review}"
fi
if [ "$role" = reviewer ]; then
  decision="$dir/decisions/$review"
  [ -f "$decision" ] || decision="$dir/decisions/reviewer"
  if [ -s "$decision" ]; then echo "$review $(cat "$decision")" >> "$dir/decided.log"; fi
fi
sleep 0.2
envelope="$dir/envelopes/$review-$role.json"
[ -f "$envelope" ] || envelope="$dir/envelopes/$role.json"
sed "s/__REVIEW_ID__/$review/g" "$envelope"
if [ -f "$dir/hang/$review-$role" ]; then
  trap 'echo "interrupted $role $review" >> "$dir/order.log"; [ -f "$dir/interrupted/$review-$role.json" ] && { echo; sed "s/__REVIEW_ID__/$review/g" "$dir/interrupted/$review-$role.json"; }; exit 130' INT
  echo "hang $role $review" >> "$dir/order.log"
  sleep 30 >/dev/null 2>&1 &
  wait $!
fi
if [ -f "$dir/die/$review-$role" ]; then kill -KILL $$; fi
echo "end $role $review" >> "$dir/order.log"
"#;

    impl FakeClaude {
        fn new() -> Self {
            let directory = StateDirectory::new();
            let path = directory.path().join("claude");
            write_executable(&path, FAKE_CLAUDE);
            for subdirectory in [
                "envelopes",
                "decisions",
                "hang",
                "interrupted",
                "die",
                "hand_over",
                "human",
                "handed_back",
                "unreadable",
                "judged",
                "handover_refused",
                "late",
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

        /// Has the process `<review id>-<role>` give the needs-repair verdict that hands its
        /// review to a human.
        fn hand_to_human(&self, name: &str) {
            std::fs::write(self.directory.path().join("hand_over").join(name), "")
                .unwrap_or_default();
        }

        /// Has the process `<review id>-<role>` give the verdict that leaves the review `other`, not
        /// its own, waiting for a human.
        fn hand_other_to_human(&self, name: &str, other: CrawlerReviewId) {
            std::fs::write(
                self.directory.path().join("hand_over").join(name),
                other.to_string(),
            )
            .unwrap_or_default();
        }

        /// Keeps the review `review_id` out of the queue until a process of the run has started,
        /// as a review that opens while the run works.
        fn appears_during_the_run(&self, review_id: CrawlerReviewId) {
            std::fs::write(
                self.directory
                    .path()
                    .join("late")
                    .join(review_id.to_string()),
                "",
            )
            .unwrap_or_default();
        }

        /// Hands the review `review_id` back to the agents now, as a human's change or verdict
        /// does: it no longer waits for a human, the queue lists when it was handed back, and no
        /// process hands it over again.
        fn hand_back(&self, review_id: CrawlerReviewId) {
            let review_id = review_id.to_string();
            let _ = std::fs::remove_file(self.directory.path().join("human").join(&review_id));
            std::fs::write(
                self.directory.path().join("handed_back").join(&review_id),
                OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
            )
            .unwrap_or_default();
            for role in ["author", "reviewer"] {
                let _ = std::fs::remove_file(
                    self.directory
                        .path()
                        .join("hand_over")
                        .join(format!("{review_id}-{role}")),
                );
            }
        }

        /// Marks the review `review_id` as one whose current candidate the reviewer already
        /// judged with a needs-repair verdict.
        fn judged(&self, review_id: CrawlerReviewId) {
            std::fs::write(
                self.directory
                    .path()
                    .join("judged")
                    .join(review_id.to_string()),
                "",
            )
            .unwrap_or_default();
        }

        /// Has the console refuse to hand the review `review_id` to a human.
        fn refuse_handover(&self, review_id: CrawlerReviewId) {
            std::fs::write(
                self.directory
                    .path()
                    .join("handover_refused")
                    .join(review_id.to_string()),
                "",
            )
            .unwrap_or_default();
        }

        /// The `<review id> <notes>` lines of the hand-overs the fake console took.
        fn handovers(&self) -> Vec<String> {
            std::fs::read_to_string(self.directory.path().join("handovers.log"))
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
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

        /// Has the hanging process `<review id>-<role>` print `envelope` when SIGINT interrupts it.
        fn report_on_interrupt(&self, name: &str, envelope: &Value) {
            std::fs::write(
                self.directory
                    .path()
                    .join("interrupted")
                    .join(format!("{name}.json")),
                envelope.to_string(),
            )
            .unwrap_or_default();
        }

        /// Waits until the fake logged `line` to its order log.
        async fn logged(&self, line: &str) {
            for _ in 0..200 {
                if self.order().iter().any(|logged| logged == line) {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            panic!("the fake claude never logged {line}: {:?}", self.order());
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
        run_once_unlocked(settings)
            .await
            .unwrap_or_else(|error| panic!("{error}: {error:?}"))
    }

    /// Runs the runner, waiting out a lock that only a sibling test holds. A run releases the
    /// lock when it ends, but a child another test thread forks in that moment holds a copy of
    /// the lock file's descriptor, and with it the `flock`, until it execs: the `pre_exec` hook
    /// makes every spawn fork, then exec. A run right after another on the same state directory
    /// can therefore find the lock held for a moment under parallel tests. A locked run returns
    /// before it does anything else, so running again is safe; a test that expects the lock to
    /// be held calls [`run`] itself.
    async fn run_once_unlocked(settings: &RunnerSettings) -> Result<RunOutcome, RunnerError> {
        for _ in 0..LOCKED_RETRIES {
            match run(settings, &RunnerStop::default()).await {
                Ok(RunOutcome::Locked) => {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                outcome => return outcome,
            }
        }
        run(settings, &RunnerStop::default()).await
    }

    /// How often [`run_once_unlocked`] runs again while the lock is held, 20 ms apart.
    const LOCKED_RETRIES: usize = 250;

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
        assert_eq!(ledger[0]["prompt_version"], "author.v7");
        assert_eq!(ledger[1]["prompt_version"], "reviewer.v5");
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
        fixture
            .claude
            .hand_to_human(&format!("{}-author", handed_over.review_id));
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

    /// An author that finds nothing to stage on a candidate the reviewer already judged gives the
    /// reviewer nothing new: the review goes to a human at once with the author's reasoning, no
    /// reviewer starts, and the review gets no cooldown. On a candidate the reviewer has not judged
    /// the reviewer runs as before.
    #[tokio::test]
    async fn should_hand_a_review_to_a_human_when_the_author_stages_nothing_on_a_judged_candidate()
    {
        let judged = review("PENDING_REVIEW", true);
        let unjudged = review("PENDING_REVIEW", true);
        let fixture = Fixture::new(vec![judged.clone(), unjudged.clone()]).await;
        fixture.claude.judged(judged.review_id);
        fixture
            .claude
            .answer("author", author_envelope("NOTHING_TO_STAGE", 0.5));

        let run = summary(&run_with(&fixture.settings(&[], &[])).await);

        assert_eq!(
            (run.worked, run.processes, run.handed_to_human, run.decided),
            (2, 3, 1, 1)
        );
        let (judged_id, unjudged_id) = (judged.review_id, unjudged.review_id);
        assert_eq!(
            fixture.claude.order(),
            [
                format!("start author {judged_id}"),
                format!("end author {judged_id}"),
                format!("start author {unjudged_id}"),
                format!("end author {unjudged_id}"),
                format!("start reviewer {unjudged_id}"),
                format!("end reviewer {unjudged_id}"),
            ]
        );
        let handovers = fixture.claude.handovers();
        assert_eq!(handovers.len(), 1, "{handovers:?}");
        assert!(
            handovers[0].starts_with(&judged_id.to_string())
                && handovers[0].contains("Author's reasoning: Staged and tested on /item/1."),
            "{handovers:?}"
        );
        let state = load_state(fixture.state.path()).unwrap_or_else(|e| panic!("{e}"));
        assert!(state.cooldowns.is_empty(), "{state:?}");
    }

    /// A hand-over the console does not take must not become one more reviewer round: the review
    /// cools down, and the reviewer does not run.
    #[tokio::test]
    async fn should_cool_a_review_down_when_the_console_does_not_take_the_hand_over() {
        let judged = review("PENDING_REVIEW", true);
        let fixture = Fixture::new(vec![judged.clone()]).await;
        fixture.claude.judged(judged.review_id);
        fixture.claude.refuse_handover(judged.review_id);
        fixture
            .claude
            .answer("author", author_envelope("NOTHING_TO_STAGE", 0.5));

        let run = summary(&run_with(&fixture.settings(&[], &[])).await);

        assert_eq!((run.processes, run.handed_to_human), (1, 0));
        assert_eq!(
            fixture.claude.order(),
            [
                format!("start author {}", judged.review_id),
                format!("end author {}", judged.review_id),
            ]
        );
        let state = load_state(fixture.state.path()).unwrap_or_else(|e| panic!("{e}"));
        assert!(state.cooldowns.contains_key(&judged.review_id), "{state:?}");
    }

    /// An author process that ends without a decision after its own needs-repair verdict reached
    /// the cap leaves its review waiting for a human, which gets no cooldown: the first run after a
    /// human hands it back works it, ahead of a review no agent process worked yet.
    #[tokio::test]
    async fn should_work_a_review_in_the_first_run_after_a_human_hands_it_back() {
        let handed_over = review("PENDING_REVIEW", true);
        let never_worked = review("PENDING_REVIEW", true);
        let fixture = Fixture::new(vec![handed_over.clone(), never_worked]).await;
        let author = format!("{}-author", handed_over.review_id);
        fixture.claude.hand_to_human(&author);
        fixture.claude.answer(
            &author,
            json!({
                "type": "result", "subtype": "error_max_turns", "is_error": true,
                "terminal_reason": "max_turns", "total_cost_usd": 0.75, "num_turns": 80,
            }),
        );
        let settings = fixture.settings(
            &[],
            &[("CRAWLER_REVIEW_AGENT_RUNNER_MAX_REVIEWS_PER_RUN", "1")],
        );

        let first_run = summary(&run_with(&settings).await);
        let state = load_state(fixture.state.path()).unwrap_or_else(|e| panic!("{e}"));
        fixture.claude.hand_back(handed_over.review_id);
        let second_run = summary(&run_with(&settings).await);

        assert_eq!(
            (
                first_run.worked,
                first_run.processes,
                first_run.handed_to_human
            ),
            (1, 1, 1)
        );
        assert!(state.cooldowns.is_empty(), "{state:?}");
        assert_eq!(
            (
                second_run.pending,
                second_run.cooling_down,
                second_run.worked
            ),
            (2, 0, 1),
            "the run after the hand-back works the review"
        );
        let id = handed_over.review_id;
        assert_eq!(
            fixture.claude.order(),
            [
                format!("start author {id}"),
                format!("end author {id}"),
                format!("start author {id}"),
                format!("end author {id}"),
            ]
        );
    }

    /// A reviewer process that leaves its review waiting for a human gives it no cooldown, also
    /// when the event trail the runner reads does not show the verdict that reached the cap.
    #[tokio::test]
    async fn should_not_cool_down_a_review_that_waits_for_a_human_after_the_reviewer() {
        let handed_over = review("PENDING_REVIEW", true);
        let fixture = Fixture::new(vec![handed_over.clone()]).await;
        fixture
            .claude
            .hand_to_human(&format!("{}-reviewer", handed_over.review_id));
        fixture.claude.decide("reviewer", "");

        let run = summary(&run_with(&fixture.settings(&[], &[])).await);

        assert_eq!(
            (run.worked, run.processes, run.decided, run.handed_to_human),
            (1, 2, 0, 0)
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

    /// A reviewer that reports the console's decision cap pauses the reviewers until the time the
    /// event trail gives, which a restart keeps: authors still run, no reviewer starts, and the
    /// reviewers start again once the pause ended.
    #[tokio::test]
    async fn should_pause_the_reviewers_at_the_decision_cap_while_authors_keep_running() {
        let refused = review("PENDING_REVIEW", true);
        let held_back = review("PENDING_REVIEW", true);
        let fixture = Fixture::new(vec![refused.clone(), held_back.clone()]).await;
        let mut capped = reviewer_envelope("APPROVE", "REFUSED", 0.2);
        capped["structured_output"]["server_answer"]["kind"] = json!("RATE_LIMITED");
        capped["structured_output"]["server_answer"]["failed_checks"] = json!([]);
        capped["structured_output"]["server_answer"]["detail"] = json!("AGENT_DECISION_LIMIT");
        fixture
            .claude
            .answer(&format!("{}-reviewer", refused.review_id), capped);
        fixture.claude.decide(&refused.review_id.to_string(), "");
        let cap_reached = AgentLimits::default().max_decisions_per_hour.get();
        std::fs::write(
            fixture.claude.directory.path().join("decided.log"),
            (0..cap_reached)
                .map(|_| format!("{} APPROVED\n", CrawlerReviewId::new()))
                .collect::<String>(),
        )
        .unwrap_or_default();
        let settings = fixture.settings(&[], &[]);
        let roles = |fixture: &Fixture| -> Vec<(String, String)> {
            fixture
                .ledger()
                .iter()
                .map(|record| {
                    (
                        record["review_id"].as_str().unwrap_or_default().to_owned(),
                        record["role"].as_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect()
        };
        let forget_cooldowns = |fixture: &Fixture| {
            let mut state = load_state(fixture.state.path()).unwrap_or_else(|e| panic!("{e}"));
            state.cooldowns.clear();
            save_state(fixture.state.path(), &state).unwrap_or_else(|e| panic!("{e}"));
        };

        let first_run = summary(&run_with(&settings).await);

        assert_eq!(
            (first_run.processes, first_run.reviewers_held_back),
            (3, 1),
            "two authors and the one reviewer that ran before the cap showed"
        );
        let state = load_state(fixture.state.path()).unwrap_or_else(|e| panic!("{e}"));
        let pause = state
            .reviewer_paused
            .unwrap_or_else(|| panic!("no reviewer pause: {state:?}"));
        let in_an_hour = OffsetDateTime::now_utc() + Duration::hours(1);
        assert!(
            (in_an_hour - pause.until).abs() < Duration::minutes(2),
            "the cap's oldest decisions just happened: {pause:?}"
        );
        assert_eq!(state.paused, None, "the usage-limit pause is another one");
        assert!(
            state
                .cooldowns
                .get(&refused.review_id)
                .is_some_and(|cooldown| cooldown.until > pause.until),
            "the refused review keeps its normal cooldown: {state:?}"
        );
        assert_eq!(
            state.cooldowns.get(&held_back.review_id).map(|c| c.until),
            Some(pause.until),
            "the held-back review is workable again when the reviewers resume"
        );

        forget_cooldowns(&fixture);
        let second_run = summary(&run_with(&settings).await);

        assert_eq!(
            (
                second_run.processes,
                second_run.worked,
                second_run.reviewers_held_back
            ),
            (2, 2, 2),
            "a restart keeps the pause: both authors run, no reviewer"
        );
        let after_second: Vec<_> = roles(&fixture).split_off(3);
        assert!(
            after_second.iter().all(|(_, role)| role == "author"),
            "{after_second:?}"
        );
        assert_eq!(
            load_state(fixture.state.path())
                .unwrap_or_default()
                .reviewer_paused,
            Some(pause)
        );

        let mut ended = load_state(fixture.state.path()).unwrap_or_else(|e| panic!("{e}"));
        ended.cooldowns.clear();
        ended.reviewer_paused = Some(ReviewerPause {
            until: OffsetDateTime::now_utc() - Duration::seconds(1),
        });
        save_state(fixture.state.path(), &ended).unwrap_or_else(|e| panic!("{e}"));
        let third_run = summary(&run_with(&settings).await);

        assert_eq!(
            (third_run.processes, third_run.reviewers_held_back),
            (3, 1),
            "the first review's reviewer starts once the pause ended, then shows the cap again"
        );
        let renewed = load_state(fixture.state.path())
            .unwrap_or_default()
            .reviewer_paused
            .map(|pause| pause.until);
        assert!(
            renewed.is_some_and(|until| until > OffsetDateTime::now_utc()),
            "the first review's reviewer shows the cap again against the same trail"
        );
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

    /// The runner's stop, which `systemctl stop` sends to the runner alone, starts no further
    /// process and stops the running one as its timeout does: SIGINT first, on which the CLI ends
    /// its turn with its envelope and the cost so far, which the ledger records. The review does
    /// not cool down, and the stopped run writes no digest.
    #[tokio::test]
    async fn should_stop_the_running_process_and_record_its_cost_when_the_runner_stops() {
        let running = review("PENDING_REVIEW", true);
        let fixture = Fixture::new(vec![running.clone(), review("PENDING_REVIEW", true)]).await;
        let author = format!("{}-author", running.review_id);
        fixture.claude.stream(
            &author,
            &[json!({ "type": "system", "subtype": "init", "model": "claude-opus-5-5" })],
        );
        fixture.claude.hang(&author);
        fixture.claude.report_on_interrupt(
            &author,
            &json!({
                "type": "result", "subtype": "error_during_execution", "is_error": true,
                "num_turns": 7, "total_cost_usd": 0.3,
                "usage": { "input_tokens": 70, "output_tokens": 30 },
            }),
        );
        let settings = fixture.settings(&[], &[]);
        let drain = std::time::Duration::from_millis(600);
        let stop = RunnerStop::with_drain(drain);
        let stopped_at = std::cell::Cell::new(None);

        let (outcome, ()) = tokio::join!(
            tokio::time::timeout(STOP_DEADLINE, run(&settings, &stop)),
            async {
                fixture
                    .claude
                    .logged(&format!("hang author {}", running.review_id))
                    .await;
                stopped_at.set(Some(std::time::Instant::now()));
                stop.request();
            }
        );
        let drained = stopped_at.get().map(|at| at.elapsed());

        let outcome = outcome
            .unwrap_or_else(|_| panic!("the stopped run outlasted the stop deadline"))
            .unwrap_or_else(|error| panic!("{error}: {error:?}"));
        let RunOutcome::Ran { summary, digest } = outcome else {
            panic!("the run found the lock held");
        };
        assert_eq!(
            (summary.worked, summary.processes, summary.stopped),
            (1, 1, Some(PassStop::RunnerStop))
        );
        assert_eq!(digest, None);
        let id = running.review_id;
        assert_eq!(
            fixture.claude.order(),
            [
                format!("start author {id}"),
                format!("hang author {id}"),
                format!("interrupted author {id}"),
            ]
        );
        let ledger = fixture.ledger();
        assert_eq!(ledger.len(), 1, "{ledger:?}");
        assert_eq!(ledger[0]["review_id"], id.to_string());
        assert_eq!(ledger[0]["end"], "FAILED");
        assert_eq!(
            ledger[0]["failure"],
            "stopped when the runner's stop deadline passed"
        );
        assert!(
            drained.is_some_and(|drained| drained >= drain),
            "the running process was stopped {drained:?} after the stop, before its {drain:?} drain"
        );
        assert_eq!(ledger[0]["num_turns"], 7);
        assert_eq!(ledger[0]["total_cost_usd"], 0.3);
        assert_eq!(
            ledger[0]
                .get("cost_charged")
                .cloned()
                .unwrap_or(json!(false)),
            json!(false)
        );
        let state = load_state(fixture.state.path()).unwrap_or_else(|e| panic!("{e}"));
        assert!(state.cooldowns.is_empty(), "{state:?}");
    }

    /// A service pass probes the login every interval, but tells it at info once per process.
    #[test]
    fn should_tell_the_login_at_info_once_and_at_debug_on_later_runs() {
        use std::sync::{Mutex, PoisonError};
        use tracing_subscriber::layer::SubscriberExt;

        #[derive(Clone, Default)]
        struct Levels(Arc<Mutex<Vec<tracing::Level>>>);
        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Levels {
            fn on_event(
                &self,
                event: &tracing::Event<'_>,
                _: tracing_subscriber::layer::Context<'_, S>,
            ) {
                self.0
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(*event.metadata().level());
            }
        }

        let logged = Levels::default();
        let announced = AtomicBool::new(false);
        let login = login::CliLogin {
            method: "claude.ai".to_owned(),
            organization: None,
            subscription: None,
        };
        let settings = {
            let state = StateDirectory::new();
            let env = HashMap::from([
                (
                    "CRAWLER_REVIEW_CONSOLE_URL".to_owned(),
                    "http://127.0.0.1:1".to_owned(),
                ),
                (
                    "CRAWLER_REVIEW_AGENT_TOKEN_FILE".to_owned(),
                    state.path().join("tokens.json").display().to_string(),
                ),
                (
                    "CRAWLER_REVIEW_AGENT_RUNNER_STATE_DIR".to_owned(),
                    state.path().display().to_string(),
                ),
                ("HOME".to_owned(), state.path().display().to_string()),
            ]);
            RunnerSettings::from_args_and_env(
                Vec::<String>::new(),
                |name| env.get(name).cloned(),
                &RunnerLocation {
                    working_dir: state.path().to_path_buf(),
                    executable: state.path().join("review-agent-runner"),
                },
            )
            .unwrap_or_else(|error| panic!("{error}"))
        };

        tracing::subscriber::with_default(
            tracing_subscriber::registry().with(logged.clone()),
            || {
                tracing::callsite::rebuild_interest_cache();
                for _ in 0..3 {
                    log_login(&announced, &settings, &login, &AgentLimits::default());
                }
            },
        );

        let levels = logged.0.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(
            *levels,
            [
                tracing::Level::INFO,
                tracing::Level::DEBUG,
                tracing::Level::DEBUG
            ]
        );
    }

    #[tokio::test]
    async fn should_end_the_service_between_runs_when_the_runner_stops() {
        let fixture = Fixture::new(vec![]).await;
        let settings = fixture.settings(&[], &[]);
        let stop = RunnerStop::default();

        let (service, ()) = tokio::join!(
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                run_every_interval(&settings, &stop)
            ),
            async {
                for _ in 0..100 {
                    let state = load_state(fixture.state.path()).ok();
                    if state.and_then(|state| state.last_digest).is_some() {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                stop.request();
            }
        );

        assert!(
            service.is_ok(),
            "the service rested its interval of {} seconds",
            settings.interval.as_secs()
        );
        let state = load_state(fixture.state.path()).unwrap_or_else(|e| panic!("{e}"));
        assert!(state.last_digest.is_some(), "the first run worked");
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
        let ledger = fixture.ledger();
        let trail_verdict = |review: &CrawlerReview, role: &str| {
            ledger
                .iter()
                .find(|record| {
                    record["review_id"] == review.review_id.to_string() && record["role"] == role
                })
                .map(|record| record["trail_verdict"].clone())
        };
        assert_eq!(
            trail_verdict(&repaired, "reviewer"),
            Some(json!("REPAIR_REQUESTED"))
        );
        assert_eq!(
            trail_verdict(&unrecorded, "reviewer"),
            Some(json!("NO_VERDICT"))
        );
        assert_eq!(trail_verdict(&repaired, "author"), Some(Value::Null));
    }

    /// The ledger and the digest show the verdict the trail records for a reviewer whose report
    /// says `NOT_SENT`, and the report's claim only as its account.
    #[tokio::test]
    async fn should_show_the_trail_s_verdict_not_the_reports_not_sent_in_the_ledger_and_digest() {
        let approved = review("PENDING_REVIEW", true);
        let fixture = Fixture::new(vec![approved.clone()]).await;
        fixture
            .claude
            .answer("reviewer", reviewer_envelope("APPROVE", "NOT_SENT", 0.25));

        let run = summary(&run_with(&fixture.settings(&[], &[])).await);

        assert_eq!(run.decided, 1);
        let reviewer = fixture
            .ledger()
            .into_iter()
            .find(|record| record["role"] == "reviewer")
            .unwrap_or_else(|| panic!("no reviewer record"));
        assert_eq!(reviewer["trail_verdict"], "APPROVED");
        assert_eq!(reviewer["report"]["server_answer"]["outcome"], "NOT_SENT");
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

    /// The pool refills from the queue whenever a slot frees: a review that opened while the run
    /// worked is worked in the same run, within the run's budget and once per review.
    #[tokio::test]
    async fn should_refill_a_freed_slot_from_the_queue_with_a_review_that_opened_during_the_run() {
        let first = review("PENDING_REVIEW", true);
        let late = review("PENDING_REVIEW", true);
        let fixture = Fixture::new(vec![first.clone(), late.clone()]).await;
        fixture.claude.appears_during_the_run(late.review_id);
        let settings = fixture.settings(
            &[],
            &[("CRAWLER_REVIEW_AGENT_RUNNER_MAX_REVIEWS_PER_RUN", "4")],
        );

        let run = summary(&run_with(&settings).await);

        assert_eq!((run.pending, run.worked, run.processes), (1, 2, 4));
        let (first_id, late_id) = (first.review_id, late.review_id);
        assert_eq!(
            fixture.claude.order(),
            [
                format!("start author {first_id}"),
                format!("end author {first_id}"),
                format!("start reviewer {first_id}"),
                format!("end reviewer {first_id}"),
                format!("start author {late_id}"),
                format!("end author {late_id}"),
                format!("start reviewer {late_id}"),
                format!("end reviewer {late_id}"),
            ]
        );
    }

    /// A refill reads the queue as it is then: a review that came to wait for a human while
    /// another one was worked gets no process.
    #[tokio::test]
    async fn should_not_start_a_review_that_came_to_wait_for_a_human_before_its_slot_freed() {
        let first = review("PENDING_REVIEW", true);
        let waiting_by_then = review("PENDING_REVIEW", true);
        let fixture = Fixture::new(vec![first.clone(), waiting_by_then.clone()]).await;
        fixture.claude.hand_other_to_human(
            &format!("{}-author", first.review_id),
            waiting_by_then.review_id,
        );

        let run = summary(&run_with(&fixture.settings(&[], &[])).await);

        assert_eq!((run.worked, run.processes), (1, 2));
        assert!(
            !fixture
                .claude
                .order()
                .iter()
                .any(|line| line.ends_with(&waiting_by_then.review_id.to_string())),
            "{:?}",
            fixture.claude.order()
        );
    }

    /// The run's budget of reviews and the once-per-run rule hold under refilling: a review the
    /// agents leave workable, with a needs-repair verdict, is not worked again in the same run.
    #[tokio::test]
    async fn should_work_at_most_the_budget_of_reviews_and_each_once_per_run() {
        let fixture = Fixture::new((0..4).map(|_| review("PENDING_REVIEW", true)).collect()).await;
        fixture.claude.decide("reviewer", "REPAIR_REQUESTED");
        let settings = fixture.settings(
            &[],
            &[
                ("CRAWLER_REVIEW_AGENT_RUNNER_MAX_REVIEWS_PER_RUN", "3"),
                ("CRAWLER_REVIEW_AGENT_RUNNER_MAX_PROCESSES", "2"),
            ],
        );

        let run = summary(&run_with(&settings).await);

        assert_eq!((run.worked, run.processes), (3, 6));
        let order = fixture.claude.order();
        let authors: std::collections::BTreeSet<&String> = order
            .iter()
            .filter(|line| line.starts_with("start author"))
            .collect();
        assert_eq!(authors.len(), 3, "{order:?}");
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

    /// The author's prompt asks for the label minimum the console applies, not for one in the
    /// runner's own environment; a run that cannot read the console's limits starts no agent.
    #[tokio::test]
    async fn should_tell_the_agents_the_console_s_limits_and_run_none_without_them() {
        let fixture = Fixture::new(vec![review("PENDING_REVIEW", true)]).await;
        let settings = fixture.settings(
            &["--once"],
            &[("CRAWLER_REVIEW_AGENT_MIN_LABELLED_URLS", "4")],
        );

        run_with(&settings).await;
        std::fs::write(
            fixture.claude.directory.path().join("limits_unreadable"),
            "",
        )
        .unwrap_or_default();
        let without_limits = run_once_unlocked(&settings).await;

        let calls = fixture.claude.calls();
        assert_eq!(
            calls.len(),
            2,
            "one author and one reviewer of the first run"
        );
        let minimum = format!(
            "at least {CONSOLE_MIN_LABELLED_URLS} URLs PRODUCT and at least {CONSOLE_MIN_LABELLED_URLS} NOT_PRODUCT"
        );
        assert!(
            calls[0]["stdin"].contains(&minimum),
            "{}",
            calls[0]["stdin"]
        );
        assert!(matches!(without_limits, Err(RunnerError::AgentLimits(_))));
    }

    #[tokio::test]
    async fn should_refuse_to_run_without_the_token_file_or_a_subscription_login() {
        let fixture = Fixture::new(vec![review("PENDING_REVIEW", true)]).await;
        let settings = fixture.settings(&[], &[]);
        fixture
            .claude
            .login(json!({ "loggedIn": false, "authMethod": "none" }));

        let logged_out = run_once_unlocked(&settings).await;
        std::fs::remove_file(&settings.console.token_file).unwrap_or_default();
        let no_token_file = run_once_unlocked(&settings).await;

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

        let outcome = run(&settings, &RunnerStop::default())
            .await
            .unwrap_or_else(|error| panic!("{error}: {error:?}"));

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
        let never_stopped = RunnerStop::default();

        let (service, ()) = tokio::join!(
            tokio::time::timeout(
                std::time::Duration::from_millis(2500),
                run_every_interval(&settings, &never_stopped)
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
