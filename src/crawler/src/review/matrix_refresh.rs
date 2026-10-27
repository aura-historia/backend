//! Background refreshes of a review's live evaluation for the review console: a product-schema
//! review's schema matrix, or a URL-pattern review's evaluation of its URL sample.
//!
//! A refresh fetches the review's pages or sample URLs live through the scraper's fetcher, which
//! paces requests by the shop's robots.txt `Crawl-delay`. That can take longer than one console
//! request may run, so a refresh runs as a task of its own: at most one per review, which
//! later requests join, and whose result the console polls for.

use crate::CrawlerReviewId;
use crate::review::http::HttpResponse;
use crate::review::model::{ReviewSchemaMatrix, UrlSampleEvaluation};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tokio::sync::watch;
use tracing::error;

/// A review's live evaluation as the review API answers it.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub(crate) enum ReviewLiveEvaluation {
    /// A product-schema review's schema matrix on its live pages.
    SchemaMatrix(ReviewSchemaMatrix),
    /// A URL-pattern review's evaluation of its URL sample.
    UrlSample(UrlSampleEvaluation),
}

/// The refreshed live evaluation, or the response that explains why the refresh failed.
pub(crate) type MatrixRefreshOutcome = Result<ReviewLiveEvaluation, HttpResponse>;

/// What a console request without `refresh` learns about a review's refresh.
pub(crate) enum MatrixRefreshStatus {
    /// No refresh is running and the last one, if any, succeeded.
    Idle,
    Running,
    /// The last refresh failed; it stays reported until the next refresh starts.
    Failed(HttpResponse),
}

enum RefreshSlot {
    Running(watch::Receiver<Option<MatrixRefreshOutcome>>),
    Failed(HttpResponse),
}

/// Registry of the refreshes started by one review server.
#[derive(Clone, Default)]
pub(crate) struct MatrixRefreshes {
    slots: Arc<Mutex<HashMap<CrawlerReviewId, RefreshSlot>>>,
}

/// A started or joined refresh.
pub(crate) struct MatrixRefreshHandle(watch::Receiver<Option<MatrixRefreshOutcome>>);

impl MatrixRefreshes {
    /// Spawns `refresh` for the review unless a refresh for it is already running, in which
    /// case `refresh` is dropped unpolled and the caller joins the running one.
    pub(crate) fn start_or_join<F>(
        &self,
        review_id: CrawlerReviewId,
        refresh: F,
    ) -> MatrixRefreshHandle
    where
        F: Future<Output = MatrixRefreshOutcome> + Send + 'static,
    {
        let mut slots = self.lock();
        if let Some(RefreshSlot::Running(running)) = slots.get(&review_id) {
            return MatrixRefreshHandle(running.clone());
        }
        let (sender, receiver) = watch::channel(None);
        slots.insert(review_id, RefreshSlot::Running(receiver.clone()));
        drop(slots);

        let registry = self.clone();
        tokio::spawn(async move {
            let outcome = match tokio::spawn(refresh).await {
                Ok(outcome) => outcome,
                Err(join_error) => {
                    error!(review_id = %review_id, error = ?join_error, "Schema matrix refresh task failed");
                    Err(HttpResponse::json(
                        500,
                        &json!({ "error": "internal server error" }),
                    ))
                }
            };
            registry.finish(review_id, &outcome);
            // Every receiver may already be gone; the registry holds the outcome for polls.
            let _ = sender.send(Some(outcome));
        });
        MatrixRefreshHandle(receiver)
    }

    pub(crate) fn status(&self, review_id: CrawlerReviewId) -> MatrixRefreshStatus {
        match self.lock().get(&review_id) {
            None => MatrixRefreshStatus::Idle,
            Some(RefreshSlot::Running(_)) => MatrixRefreshStatus::Running,
            Some(RefreshSlot::Failed(response)) => MatrixRefreshStatus::Failed(response.clone()),
        }
    }

    /// A successful refresh has stored its matrix, so only a failure needs remembering.
    fn finish(&self, review_id: CrawlerReviewId, outcome: &MatrixRefreshOutcome) {
        let mut slots = self.lock();
        match outcome {
            Ok(_) => {
                slots.remove(&review_id);
            }
            Err(response) => {
                slots.insert(review_id, RefreshSlot::Failed(response.clone()));
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<CrawlerReviewId, RefreshSlot>> {
        // The map holds no invariant a panicking holder could break halfway.
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl MatrixRefreshHandle {
    /// Waits up to `wait` for the outcome; `None` means the refresh is still running.
    pub(crate) async fn outcome_within(mut self, wait: Duration) -> Option<MatrixRefreshOutcome> {
        let finished = tokio::time::timeout(wait, self.0.wait_for(Option::is_some)).await;
        match finished {
            Ok(Ok(outcome)) => outcome.clone(),
            // The sender only drops without a value when the runtime shuts down.
            Ok(Err(_)) | Err(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::SchemaMatrix;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    fn matrix() -> ReviewLiveEvaluation {
        ReviewLiveEvaluation::SchemaMatrix(ReviewSchemaMatrix::of(
            SchemaMatrix {
                review_id: None,
                candidates: Vec::new(),
                removed_page_matches: Vec::new(),
                live_evaluation: None,
            },
            &[],
        ))
    }

    fn panicking_refresh() -> MatrixRefreshOutcome {
        panic!("refresh panicked")
    }

    #[tokio::test]
    async fn should_share_one_running_refresh_when_a_second_request_arrives() {
        let refreshes = MatrixRefreshes::default();
        let review_id = CrawlerReviewId::new();
        let started = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(Notify::new());

        let refresh = |started: Arc<AtomicUsize>, release: Arc<Notify>| async move {
            started.fetch_add(1, Ordering::SeqCst);
            release.notified().await;
            Ok(matrix())
        };
        let first = refreshes.start_or_join(review_id, refresh(started.clone(), release.clone()));
        let second = refreshes.start_or_join(review_id, refresh(started.clone(), release.clone()));
        tokio::task::yield_now().await;

        assert!(matches!(
            refreshes.status(review_id),
            MatrixRefreshStatus::Running
        ));
        release.notify_one();
        let first = first.outcome_within(Duration::from_secs(5)).await;
        let second = second.outcome_within(Duration::from_secs(5)).await;

        assert_eq!(started.load(Ordering::SeqCst), 1);
        assert!(matches!(first, Some(Ok(_))));
        assert!(matches!(second, Some(Ok(_))));
        assert!(matches!(
            refreshes.status(review_id),
            MatrixRefreshStatus::Idle
        ));
    }

    #[tokio::test]
    async fn should_report_a_running_refresh_when_it_outlasts_the_wait() {
        let refreshes = MatrixRefreshes::default();
        let review_id = CrawlerReviewId::new();
        let release = Arc::new(Notify::new());
        let waiting = release.clone();

        let handle = refreshes.start_or_join(review_id, async move {
            waiting.notified().await;
            Ok(matrix())
        });

        assert!(
            handle
                .outcome_within(Duration::from_millis(20))
                .await
                .is_none()
        );
        assert!(matches!(
            refreshes.status(review_id),
            MatrixRefreshStatus::Running
        ));
        release.notify_one();
    }

    #[tokio::test]
    async fn should_keep_a_failure_for_polls_until_the_next_refresh_starts() {
        let refreshes = MatrixRefreshes::default();
        let review_id = CrawlerReviewId::new();

        let failed = refreshes
            .start_or_join(review_id, async {
                Err(HttpResponse::json(
                    502,
                    &json!({ "error": "robots.txt disallows" }),
                ))
            })
            .outcome_within(Duration::from_secs(5))
            .await;
        assert!(matches!(failed, Some(Err(_))));
        let MatrixRefreshStatus::Failed(response) = refreshes.status(review_id) else {
            panic!("a failed refresh must stay reported");
        };
        assert!(response.to_string().contains("robots.txt disallows"));

        let retried = refreshes
            .start_or_join(review_id, async { Ok(matrix()) })
            .outcome_within(Duration::from_secs(5))
            .await;
        assert!(matches!(retried, Some(Ok(_))));
        assert!(matches!(
            refreshes.status(review_id),
            MatrixRefreshStatus::Idle
        ));
    }

    #[tokio::test]
    async fn should_report_a_panicking_refresh_as_failed() {
        let refreshes = MatrixRefreshes::default();
        let review_id = CrawlerReviewId::new();

        let outcome = refreshes
            .start_or_join(review_id, async { panicking_refresh() })
            .outcome_within(Duration::from_secs(5))
            .await;

        assert!(matches!(outcome, Some(Err(_))));
        assert!(matches!(
            refreshes.status(review_id),
            MatrixRefreshStatus::Failed(_)
        ));
    }
}
