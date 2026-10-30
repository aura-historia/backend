//! Background schema-matrix refreshes for the review console.
//!
//! A refresh fetches every review page live through the scraper's fetcher, which paces
//! requests by the shop's robots.txt `Crawl-delay`. That can take longer than one console
//! request may run, so a refresh runs as a task of its own: at most one per review, which
//! later requests join, and whose result the console polls for. Each refresh is a live job of
//! the console's [`LiveJobLimit`] until it ends: one that would go above the limit is not started,
//! while joining a running one takes no place. A failure the review refused because of the role of
//! the actor who started the refresh reaches that actor only.

use crate::CrawlerReviewId;
use crate::review::http::HttpResponse;
use crate::review::live_jobs::{LiveJobLimit, LiveJobsBusy};
use crate::review::model::{ReviewActor, ReviewSchemaMatrix};
use serde_json::json;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tokio::sync::watch;
use tracing::error;

/// The refreshed matrix, or why the refresh failed.
pub(crate) type MatrixRefreshOutcome = Result<ReviewSchemaMatrix, MatrixRefreshFailure>;

/// Why a refresh failed, and who learns it.
#[derive(Clone)]
pub(crate) enum MatrixRefreshFailure {
    /// The refresh failed for the review, such as on a page fetch: every request learns it until
    /// the next refresh starts.
    Review(HttpResponse),
    /// The review refused a review agent's refresh because it came to wait for a human while the
    /// refresh ran (`HUMAN_REQUIRED`). Only the `actor` who started it learns it; for every other
    /// actor the refresh stored nothing.
    HumanRequired {
        actor: ReviewActor,
        response: HttpResponse,
    },
}

impl From<HttpResponse> for MatrixRefreshFailure {
    fn from(response: HttpResponse) -> Self {
        Self::Review(response)
    }
}

impl MatrixRefreshFailure {
    /// The answer `asking` gets, or `None` when the failure concerns another actor only.
    fn answer_for(&self, asking: ReviewActor) -> Option<HttpResponse> {
        match self {
            Self::Review(response) => Some(response.clone()),
            Self::HumanRequired { actor, response } => (*actor == asking).then(|| response.clone()),
        }
    }
}

/// What a console request without `refresh` learns about a review's refresh.
pub(crate) enum MatrixRefreshStatus {
    /// No refresh is running and the last one, if any, succeeded or failed for another actor.
    Idle,
    Running,
    /// The last refresh failed; it stays reported until the next refresh starts.
    Failed(HttpResponse),
}

enum RefreshSlot {
    Running(watch::Receiver<Option<MatrixRefreshOutcome>>),
    Failed(MatrixRefreshFailure),
}

/// Registry of the refreshes started by one review server.
#[derive(Clone, Default)]
pub(crate) struct MatrixRefreshes {
    slots: Arc<Mutex<HashMap<CrawlerReviewId, RefreshSlot>>>,
}

/// A refresh `actor` started or joined.
pub(crate) struct MatrixRefreshHandle {
    outcome: watch::Receiver<Option<MatrixRefreshOutcome>>,
    actor: ReviewActor,
}

impl MatrixRefreshes {
    /// Spawns `actor`'s `refresh` for the review as a live job of `live_jobs`, unless a refresh for
    /// it is already running, in which case `refresh` is dropped unpolled and the caller joins the
    /// running one. A new refresh above the limit is refused, and `refresh` dropped unpolled.
    pub(crate) fn start_or_join<F>(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
        live_jobs: &LiveJobLimit,
        refresh: F,
    ) -> Result<MatrixRefreshHandle, LiveJobsBusy>
    where
        F: Future<Output = MatrixRefreshOutcome> + Send + 'static,
    {
        let mut slots = self.lock();
        if let Some(RefreshSlot::Running(running)) = slots.get(&review_id) {
            return Ok(MatrixRefreshHandle {
                outcome: running.clone(),
                actor,
            });
        }
        let live_job = live_jobs.try_start()?;
        let (sender, receiver) = watch::channel(None);
        slots.insert(review_id, RefreshSlot::Running(receiver.clone()));
        drop(slots);

        let registry = self.clone();
        tokio::spawn(async move {
            let outcome = match tokio::spawn(refresh).await {
                Ok(outcome) => outcome,
                Err(join_error) => {
                    error!(review_id = %review_id, error = ?join_error, "Schema matrix refresh task failed");
                    Err(MatrixRefreshFailure::Review(HttpResponse::json(
                        500,
                        &json!({ "error": "internal server error" }),
                    )))
                }
            };
            // The job ends with its outcome, so whoever learns the outcome finds its place free.
            drop(live_job);
            registry.finish(review_id, &outcome);
            // Every receiver may already be gone; the registry holds the outcome for polls.
            let _ = sender.send(Some(outcome));
        });
        Ok(MatrixRefreshHandle {
            outcome: receiver,
            actor,
        })
    }

    /// The review's refresh as `actor` learns it.
    pub(crate) fn status(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
    ) -> MatrixRefreshStatus {
        match self.lock().get(&review_id) {
            None => MatrixRefreshStatus::Idle,
            Some(RefreshSlot::Running(_)) => MatrixRefreshStatus::Running,
            Some(RefreshSlot::Failed(failure)) => failure
                .answer_for(actor)
                .map_or(MatrixRefreshStatus::Idle, MatrixRefreshStatus::Failed),
        }
    }

    /// A successful refresh has stored its matrix, so only a failure needs remembering.
    fn finish(&self, review_id: CrawlerReviewId, outcome: &MatrixRefreshOutcome) {
        let mut slots = self.lock();
        match outcome {
            Ok(_) => {
                slots.remove(&review_id);
            }
            Err(failure) => {
                slots.insert(review_id, RefreshSlot::Failed(failure.clone()));
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<CrawlerReviewId, RefreshSlot>> {
        // The map holds no invariant a panicking holder could break halfway.
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl MatrixRefreshHandle {
    /// Waits up to `wait` for the outcome as the handle's actor learns it; `None` means the
    /// refresh is still running.
    pub(crate) async fn outcome_within(
        mut self,
        wait: Duration,
    ) -> Option<Result<ReviewSchemaMatrix, HttpResponse>> {
        let finished = tokio::time::timeout(wait, self.outcome.wait_for(Option::is_some)).await;
        let outcome = match finished {
            Ok(Ok(outcome)) => outcome.clone(),
            // The sender only drops without a value when the runtime shuts down.
            Ok(Err(_)) | Err(_) => None,
        }?;
        Some(outcome.map_err(|failure| {
            failure
                .answer_for(self.actor)
                .unwrap_or_else(refused_for_the_starting_actor)
        }))
    }
}

/// Answers an actor who joined a refresh the review refused because of the role of the actor who
/// started it: it stored nothing, and the actor's own refresh is judged on its own.
fn refused_for_the_starting_actor() -> HttpResponse {
    HttpResponse::json(
        409,
        &json!({
            "error": "the matrix refresh this request joined was refused for the actor who started it and stored nothing; refresh the matrix again",
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::SchemaMatrix;
    use std::num::NonZeroU32;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    fn matrix() -> ReviewSchemaMatrix {
        ReviewSchemaMatrix::of(
            SchemaMatrix {
                review_id: None,
                candidates: Vec::new(),
                removed_page_matches: Vec::new(),
                live_evaluation: None,
            },
            &[],
        )
    }

    fn live_jobs(limit: u32) -> LiveJobLimit {
        LiveJobLimit::new(NonZeroU32::new(limit).unwrap_or(NonZeroU32::MIN))
    }

    fn started(handle: Result<MatrixRefreshHandle, LiveJobsBusy>) -> MatrixRefreshHandle {
        handle.unwrap_or_else(|busy| panic!("refresh refused: {busy:?}"))
    }

    fn panicking_refresh() -> MatrixRefreshOutcome {
        panic!("refresh panicked")
    }

    #[tokio::test]
    async fn should_share_one_running_refresh_when_a_second_request_arrives() {
        let refreshes = MatrixRefreshes::default();
        let live_jobs = live_jobs(1);
        let review_id = CrawlerReviewId::new();
        let started_refreshes = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(Notify::new());

        let refresh = |started: Arc<AtomicUsize>, release: Arc<Notify>| async move {
            started.fetch_add(1, Ordering::SeqCst);
            release.notified().await;
            Ok(matrix())
        };
        let first = started(refreshes.start_or_join(
            review_id,
            ReviewActor::Human,
            &live_jobs,
            refresh(started_refreshes.clone(), release.clone()),
        ));
        let second = started(refreshes.start_or_join(
            review_id,
            ReviewActor::AgentReviewer,
            &live_jobs,
            refresh(started_refreshes.clone(), release.clone()),
        ));
        tokio::task::yield_now().await;

        assert!(matches!(
            refreshes.status(review_id, ReviewActor::Human),
            MatrixRefreshStatus::Running
        ));
        release.notify_one();
        let first = first.outcome_within(Duration::from_secs(5)).await;
        let second = second.outcome_within(Duration::from_secs(5)).await;

        assert_eq!(started_refreshes.load(Ordering::SeqCst), 1);
        assert!(matches!(first, Some(Ok(_))));
        assert!(matches!(second, Some(Ok(_))));
        assert!(matches!(
            refreshes.status(review_id, ReviewActor::Human),
            MatrixRefreshStatus::Idle
        ));
    }

    /// A running refresh holds its place under the limit until it ends; joining it takes none.
    #[tokio::test]
    async fn should_refuse_a_new_refresh_above_the_live_job_limit_until_the_running_one_ends() {
        let refreshes = MatrixRefreshes::default();
        let live_jobs = live_jobs(1);
        let review_id = CrawlerReviewId::new();
        let other_review_id = CrawlerReviewId::new();
        let release = Arc::new(Notify::new());
        let waiting = release.clone();

        let running = started(refreshes.start_or_join(
            review_id,
            ReviewActor::Human,
            &live_jobs,
            async move {
                waiting.notified().await;
                Ok(matrix())
            },
        ));
        let joined =
            refreshes.start_or_join(review_id, ReviewActor::AgentReviewer, &live_jobs, async {
                Ok(matrix())
            });
        let refused =
            refreshes.start_or_join(other_review_id, ReviewActor::Human, &live_jobs, async {
                Ok(matrix())
            });

        assert!(joined.is_ok());
        assert_eq!(
            refused.err().map(|busy| busy.limit.get()),
            Some(1),
            "a second refresh goes above the limit"
        );
        assert!(matches!(
            refreshes.status(other_review_id, ReviewActor::Human),
            MatrixRefreshStatus::Idle
        ));
        release.notify_one();
        assert!(matches!(
            running.outcome_within(Duration::from_secs(5)).await,
            Some(Ok(_))
        ));
        tokio::task::yield_now().await;
        let after = started(refreshes.start_or_join(
            other_review_id,
            ReviewActor::Human,
            &live_jobs,
            async { Ok(matrix()) },
        ));
        assert!(matches!(
            after.outcome_within(Duration::from_secs(5)).await,
            Some(Ok(_))
        ));
    }

    #[tokio::test]
    async fn should_report_a_running_refresh_when_it_outlasts_the_wait() {
        let refreshes = MatrixRefreshes::default();
        let review_id = CrawlerReviewId::new();
        let release = Arc::new(Notify::new());
        let waiting = release.clone();

        let handle = started(refreshes.start_or_join(
            review_id,
            ReviewActor::Human,
            &live_jobs(1),
            async move {
                waiting.notified().await;
                Ok(matrix())
            },
        ));

        assert!(
            handle
                .outcome_within(Duration::from_millis(20))
                .await
                .is_none()
        );
        assert!(matches!(
            refreshes.status(review_id, ReviewActor::Human),
            MatrixRefreshStatus::Running
        ));
        release.notify_one();
    }

    #[tokio::test]
    async fn should_keep_a_failure_for_polls_until_the_next_refresh_starts() {
        let refreshes = MatrixRefreshes::default();
        let live_jobs = live_jobs(1);
        let review_id = CrawlerReviewId::new();

        let failed =
            started(
                refreshes.start_or_join(review_id, ReviewActor::Human, &live_jobs, async {
                    Err(HttpResponse::json(502, &json!({ "error": "robots.txt disallows" })).into())
                }),
            )
            .outcome_within(Duration::from_secs(5))
            .await;
        assert!(matches!(failed, Some(Err(_))));
        for actor in [ReviewActor::Human, ReviewActor::AgentAuthor] {
            let MatrixRefreshStatus::Failed(response) = refreshes.status(review_id, actor) else {
                panic!("a failed refresh must stay reported to {actor:?}");
            };
            assert!(response.to_string().contains("robots.txt disallows"));
        }

        let retried = started(refreshes.start_or_join(
            review_id,
            ReviewActor::Human,
            &live_jobs,
            async { Ok(matrix()) },
        ))
        .outcome_within(Duration::from_secs(5))
        .await;
        assert!(matches!(retried, Some(Ok(_))));
        assert!(matches!(
            refreshes.status(review_id, ReviewActor::Human),
            MatrixRefreshStatus::Idle
        ));
    }

    /// A review agent's refresh refused because the review came to wait for a human is that
    /// agent's answer only: a human who joined it or reads the review afterwards never gets the
    /// text written for the agents.
    #[tokio::test]
    async fn should_answer_a_refusal_of_the_starting_actor_to_that_actor_only() {
        let refreshes = MatrixRefreshes::default();
        let live_jobs = live_jobs(1);
        let review_id = CrawlerReviewId::new();
        let release = Arc::new(Notify::new());
        let waiting = release.clone();
        const AGENT_TEXT: &str = "leave it until a human changes or judges it";

        let agent = started(refreshes.start_or_join(
            review_id,
            ReviewActor::AgentReviewer,
            &live_jobs,
            async move {
                waiting.notified().await;
                Err(MatrixRefreshFailure::HumanRequired {
                    actor: ReviewActor::AgentReviewer,
                    response: HttpResponse::json(
                        409,
                        &json!({ "error": AGENT_TEXT, "reason": "HUMAN_REQUIRED" }),
                    ),
                })
            },
        ));
        let human = started(refreshes.start_or_join(
            review_id,
            ReviewActor::Human,
            &live_jobs,
            async { Ok(matrix()) },
        ));
        release.notify_one();
        let agent = agent.outcome_within(Duration::from_secs(5)).await;
        let human = human.outcome_within(Duration::from_secs(5)).await;

        let Some(Err(agent)) = agent else {
            panic!("the agent's refresh must fail");
        };
        assert!(agent.to_string().contains(AGENT_TEXT), "{agent}");
        let Some(Err(human)) = human else {
            panic!("the joined refresh stored nothing");
        };
        assert!(human.to_string().starts_with("HTTP/1.1 409"), "{human}");
        assert!(!human.to_string().contains(AGENT_TEXT), "{human}");
        assert!(matches!(
            refreshes.status(review_id, ReviewActor::AgentReviewer),
            MatrixRefreshStatus::Failed(_)
        ));
        for actor in [ReviewActor::Human, ReviewActor::AgentAuthor] {
            assert!(
                matches!(
                    refreshes.status(review_id, actor),
                    MatrixRefreshStatus::Idle
                ),
                "{actor:?}"
            );
        }
    }

    #[tokio::test]
    async fn should_report_a_panicking_refresh_as_failed() {
        let refreshes = MatrixRefreshes::default();
        let review_id = CrawlerReviewId::new();

        let outcome = started(refreshes.start_or_join(
            review_id,
            ReviewActor::Human,
            &live_jobs(1),
            async { panicking_refresh() },
        ))
        .outcome_within(Duration::from_secs(5))
        .await;

        assert!(matches!(outcome, Some(Err(_))));
        assert!(matches!(
            refreshes.status(review_id, ReviewActor::Human),
            MatrixRefreshStatus::Failed(_)
        ));
    }
}
