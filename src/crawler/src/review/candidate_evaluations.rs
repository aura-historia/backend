//! Background evaluations of product-schema candidates proposed for a review, which the review
//! console answers without storing them.
//!
//! An evaluation fetches the review's pages and the proposed URLs live through the scraper's
//! fetcher, at the shop's robots.txt `Crawl-delay`, so like a matrix refresh it can outlast one
//! console request. It runs as a task of its own under an id the console answers with, and its
//! outcome stays available for polling for a while after it finished. Unlike a matrix refresh,
//! nothing is stored and no later request joins it: each proposal is its own evaluation.

use crate::CrawlerReviewId;
use crate::review::http::HttpResponse;
use crate::review::model::SchemaMatrix;
use serde_json::json;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tracing::error;

/// The evaluated matrix, or the response that explains why the evaluation failed.
pub(crate) type CandidateEvaluationOutcome = Result<SchemaMatrix, HttpResponse>;

/// How many evaluations run at once; each fetches several pages at a shop's crawl delay.
const MAX_RUNNING_EVALUATIONS: usize = 4;
/// How many finished evaluations stay available for polling at most.
const MAX_FINISHED_EVALUATIONS: usize = 32;
/// How long a finished evaluation stays available for polling.
const FINISHED_EVALUATION_TTL: Duration = Duration::from_secs(10 * 60);

struct EvaluationSlot {
    review_id: CrawlerReviewId,
    outcome: watch::Receiver<Option<CandidateEvaluationOutcome>>,
    finished: Option<Instant>,
}

/// The evaluations started by one review server.
#[derive(Clone, Default)]
pub(crate) struct CandidateEvaluations {
    slots: Arc<Mutex<HashMap<uuid::Uuid, EvaluationSlot>>>,
}

/// A started or polled evaluation.
pub(crate) struct CandidateEvaluationHandle(watch::Receiver<Option<CandidateEvaluationOutcome>>);

/// [`MAX_RUNNING_EVALUATIONS`] evaluations are running already.
#[derive(Debug)]
pub(crate) struct EvaluationsBusy;

impl CandidateEvaluations {
    /// Spawns `evaluation` of a candidate proposed for the review under a new id, unless
    /// [`MAX_RUNNING_EVALUATIONS`] are running already.
    pub(crate) fn start<F>(
        &self,
        review_id: CrawlerReviewId,
        evaluation: F,
    ) -> Result<(uuid::Uuid, CandidateEvaluationHandle), EvaluationsBusy>
    where
        F: Future<Output = CandidateEvaluationOutcome> + Send + 'static,
    {
        let mut slots = self.lock();
        forget_expired(&mut slots, Instant::now());
        if slots
            .values()
            .filter(|slot| slot.finished.is_none())
            .count()
            >= MAX_RUNNING_EVALUATIONS
        {
            return Err(EvaluationsBusy);
        }
        let evaluation_id = uuid::Uuid::new_v4();
        let (sender, receiver) = watch::channel(None);
        slots.insert(
            evaluation_id,
            EvaluationSlot {
                review_id,
                outcome: receiver.clone(),
                finished: None,
            },
        );
        drop(slots);

        let registry = self.clone();
        tokio::spawn(async move {
            let outcome = match tokio::spawn(evaluation).await {
                Ok(outcome) => outcome,
                Err(join_error) => {
                    error!(review_id = %review_id, error = ?join_error, "Candidate evaluation task failed");
                    Err(HttpResponse::json(
                        500,
                        &json!({ "error": "internal server error" }),
                    ))
                }
            };
            // Every receiver may already be gone; the registry's own receiver keeps the outcome.
            let _ = sender.send(Some(outcome));
            registry.finish(evaluation_id);
        });
        Ok((evaluation_id, CandidateEvaluationHandle(receiver)))
    }

    /// The evaluation `evaluation_id` of the review, while it runs or stays available; `None` for
    /// an unknown, another review's or a forgotten one.
    pub(crate) fn find(
        &self,
        review_id: CrawlerReviewId,
        evaluation_id: uuid::Uuid,
    ) -> Option<CandidateEvaluationHandle> {
        let mut slots = self.lock();
        forget_expired(&mut slots, Instant::now());
        slots
            .get(&evaluation_id)
            .filter(|slot| slot.review_id == review_id)
            .map(|slot| CandidateEvaluationHandle(slot.outcome.clone()))
    }

    fn finish(&self, evaluation_id: uuid::Uuid) {
        if let Some(slot) = self.lock().get_mut(&evaluation_id) {
            slot.finished = Some(Instant::now());
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<uuid::Uuid, EvaluationSlot>> {
        // The map holds no invariant a panicking holder could break halfway.
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Forgets the finished evaluations older than [`FINISHED_EVALUATION_TTL`], and the oldest ones
/// beyond [`MAX_FINISHED_EVALUATIONS`].
fn forget_expired(slots: &mut HashMap<uuid::Uuid, EvaluationSlot>, now: Instant) {
    slots.retain(|_, slot| {
        slot.finished
            .is_none_or(|finished| now.duration_since(finished) < FINISHED_EVALUATION_TTL)
    });
    let mut finished: Vec<(Instant, uuid::Uuid)> = slots
        .iter()
        .filter_map(|(id, slot)| slot.finished.map(|finished| (finished, *id)))
        .collect();
    if finished.len() > MAX_FINISHED_EVALUATIONS {
        finished.sort_unstable();
        for (_, id) in &finished[..finished.len() - MAX_FINISHED_EVALUATIONS] {
            slots.remove(id);
        }
    }
}

impl CandidateEvaluationHandle {
    /// Waits up to `wait` for the outcome; `None` means the evaluation is still running.
    pub(crate) async fn outcome_within(
        mut self,
        wait: Duration,
    ) -> Option<CandidateEvaluationOutcome> {
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
    use tokio::sync::oneshot;

    fn matrix(review_id: CrawlerReviewId) -> SchemaMatrix {
        SchemaMatrix {
            review_id: Some(review_id),
            candidates: Vec::new(),
            removed_page_matches: Vec::new(),
            live_evaluation: None,
        }
    }

    #[tokio::test]
    async fn should_answer_the_outcome_on_poll_once_the_evaluation_finishes() {
        let evaluations = CandidateEvaluations::default();
        let review_id = CrawlerReviewId::new();
        let (release, released) = oneshot::channel::<()>();

        let (evaluation_id, started) = evaluations
            .start(review_id, async move {
                let _ = released.await;
                Ok(matrix(review_id))
            })
            .unwrap_or_else(|_| panic!("evaluations busy"));

        assert!(
            started
                .outcome_within(Duration::from_millis(10))
                .await
                .is_none()
        );
        let _ = release.send(());
        let polled = evaluations
            .find(review_id, evaluation_id)
            .unwrap_or_else(|| panic!("evaluation forgotten"))
            .outcome_within(Duration::from_secs(5))
            .await;
        assert!(matches!(polled, Some(Ok(matrix)) if matrix.review_id == Some(review_id)));
        assert!(
            evaluations
                .find(CrawlerReviewId::new(), evaluation_id)
                .is_none()
        );
        assert!(evaluations.find(review_id, uuid::Uuid::new_v4()).is_none());
    }

    #[tokio::test]
    async fn should_refuse_an_evaluation_while_the_maximum_runs() {
        let evaluations = CandidateEvaluations::default();
        let review_id = CrawlerReviewId::new();
        let mut releases = Vec::new();
        for _ in 0..MAX_RUNNING_EVALUATIONS {
            let (release, released) = oneshot::channel::<()>();
            releases.push(release);
            assert!(
                evaluations
                    .start(review_id, async move {
                        let _ = released.await;
                        Ok(matrix(review_id))
                    })
                    .is_ok()
            );
        }

        assert!(
            evaluations
                .start(review_id, async move { Ok(matrix(review_id)) })
                .is_err()
        );
        drop(releases);
    }

    #[test]
    fn should_forget_finished_evaluations_after_their_time_and_beyond_the_maximum() {
        let start = Instant::now();
        let review_id = CrawlerReviewId::new();
        let slot = |finished: Option<Instant>| EvaluationSlot {
            review_id,
            outcome: watch::channel(None).1,
            finished,
        };
        let mut slots = HashMap::new();
        let running = uuid::Uuid::new_v4();
        let expired = uuid::Uuid::new_v4();
        slots.insert(running, slot(None));
        slots.insert(expired, slot(Some(start)));
        // Finished one second apart, the first one oldest, all within their time.
        let recent: Vec<uuid::Uuid> = (0..=MAX_FINISHED_EVALUATIONS)
            .map(|second| {
                let id = uuid::Uuid::new_v4();
                let finished = start + Duration::from_secs(10 + u64::try_from(second).unwrap_or(0));
                slots.insert(id, slot(Some(finished)));
                id
            })
            .collect();

        forget_expired(
            &mut slots,
            start + FINISHED_EVALUATION_TTL + Duration::from_secs(5),
        );

        assert!(slots.contains_key(&running));
        assert!(!slots.contains_key(&expired));
        assert!(!slots.contains_key(&recent[0]));
        assert!(recent[1..].iter().all(|id| slots.contains_key(id)));
        assert_eq!(slots.len(), MAX_FINISHED_EVALUATIONS + 1);
    }
}
