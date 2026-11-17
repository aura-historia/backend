//! At-least-once delivery: leases, acknowledgment, retries and the retry budget.

mod support;

use futures::future::join_all;
use platform_postgres_queue::{
    CustodyReason, Disposition, JobId, JobQueue, QueueOrdering, Settlement,
};
use std::{collections::HashSet, time::Duration};
use support::{
    DATABASE, SHORT, ack, claim_ids, claim_one, class, custody_row, push, scope, sorted, standard,
};
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_never_lease_one_job_to_two_concurrent_claimers() {
    let pool = get_postgres_client().await;
    let scope = standard("concurrent");
    let mut enqueued = Vec::new();
    for index in 0..400 {
        enqueued.push(push(&pool, &scope, None, &format!("{{\"n\":{index}}}")).await);
    }
    let queue = JobQueue::new(pool, scope);

    let claimers = (0..8).map(|_| {
        let queue = queue.clone();
        async move {
            let mut claimed = Vec::new();
            loop {
                let jobs = queue.claim(7).await.unwrap();
                if jobs.is_empty() {
                    return claimed;
                }
                claimed.extend(jobs.iter().map(|job| job.job_id()));
            }
        }
    });
    let claimed: Vec<JobId> = join_all(claimers).await.into_iter().flatten().collect();

    let unique: HashSet<_> = claimed.iter().copied().collect();
    assert_eq!(claimed.len(), unique.len(), "a job was leased twice");
    assert_eq!(sorted(enqueued), sorted(claimed));
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_redeliver_a_job_once_its_lease_expires_and_reject_the_stale_lease() {
    let pool = get_postgres_client().await;
    let scope = scope("lease", QueueOrdering::Standard, SHORT, 5, Duration::ZERO);
    let id = push(&pool, &scope, None, "{}").await;
    let queue = JobQueue::new(pool, scope);

    let first = claim_one(&queue).await.unwrap();
    assert_eq!((id, 1), (first.job_id(), first.attempt()));
    assert_eq!(
        None,
        claim_one(&queue).await,
        "a leased job is not claimable"
    );

    tokio::time::sleep(SHORT * 2).await;
    let second = claim_one(&queue).await.unwrap();

    assert_eq!((id, 2), (second.job_id(), second.attempt()));
    assert_eq!(
        Settlement::StaleLease,
        queue
            .settle(first.lease(), Disposition::Complete)
            .await
            .unwrap()
    );
    assert_eq!(
        Settlement::StaleLease,
        queue
            .settle(first.lease(), Disposition::Retry(class("late")))
            .await
            .unwrap()
    );
    ack(&queue, &second).await;
    assert_eq!(
        Settlement::StaleLease,
        queue
            .settle(second.lease(), Disposition::Complete)
            .await
            .unwrap(),
        "a repeated acknowledgment changes nothing"
    );
    assert_eq!(0, queue.inspect().await.unwrap().depth);
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_redeliver_a_job_whose_worker_crashed_between_claim_and_ack() {
    let scope = scope("crash", QueueOrdering::Standard, SHORT, 5, Duration::ZERO);
    let id = push(&get_postgres_client().await, &scope, None, "{}").await;

    // The crashed worker's claim committed; its process, pool and transaction are gone.
    let crashed = get_postgres_client().await;
    let crashed_queue = JobQueue::new(crashed.clone(), scope.clone());
    let lost = claim_one(&crashed_queue).await.unwrap();
    let mut effect = crashed.begin().await.unwrap();
    sqlx::query("SELECT 1").execute(&mut *effect).await.unwrap();
    drop(effect);
    crashed.close().await;

    let survivor = JobQueue::new(get_postgres_client().await, scope);
    assert_eq!(None, claim_one(&survivor).await);
    tokio::time::sleep(SHORT * 2).await;
    let redelivered = claim_one(&survivor).await.unwrap();

    assert_eq!(id, lost.job_id());
    assert_eq!((id, 2), (redelivered.job_id(), redelivered.attempt()));
    ack(&survivor, &redelivered).await;
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_release_a_retried_job_only_after_its_backoff() {
    let pool = get_postgres_client().await;
    let scope = scope(
        "backoff",
        QueueOrdering::Standard,
        Duration::from_secs(300),
        5,
        SHORT,
    );
    let id = push(&pool, &scope, None, "{}").await;
    let queue = JobQueue::new(pool, scope);

    let first = claim_one(&queue).await.unwrap();
    assert_eq!(
        Settlement::Released,
        queue
            .settle(
                first.lease(),
                Disposition::Retry(class("provider_unavailable"))
            )
            .await
            .unwrap()
    );
    assert_eq!(None, claim_one(&queue).await, "the backoff has not passed");
    tokio::time::sleep(SHORT * 2).await;

    let second = claim_one(&queue).await.unwrap();
    assert_eq!((id, 2), (second.job_id(), second.attempt()));
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_move_a_job_unchanged_to_custody_once_its_retry_budget_is_spent() {
    let pool = get_postgres_client().await;
    let scope = scope(
        "budget",
        QueueOrdering::Standard,
        Duration::from_secs(300),
        3,
        Duration::ZERO,
    );
    let payload = r#"{"body":"kept byte for byte",  "spacing":true}"#;
    let id = push(&pool, &scope, None, payload).await;
    let queue = JobQueue::new(pool.clone(), scope);

    for attempt in 1..=3 {
        let job = claim_one(&queue).await.unwrap();
        assert_eq!(attempt, job.attempt());
        let expected = if attempt < 3 {
            Settlement::Released
        } else {
            Settlement::MovedToCustody(CustodyReason::RetryBudgetExhausted)
        };
        let settlement = queue
            .settle(job.lease(), Disposition::Retry(class("pool_timed_out")))
            .await
            .unwrap();
        assert_eq!(expected, settlement);
    }

    assert_eq!(None, claim_one(&queue).await);
    let stats = queue.inspect().await.unwrap();
    assert_eq!((0, 1), (stats.depth, stats.in_custody));
    assert_eq!(
        (
            payload.to_owned(),
            3,
            "RETRY_BUDGET_EXHAUSTED".to_owned(),
            Some("pool_timed_out".to_owned())
        ),
        custody_row(&pool, id).await
    );
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_move_a_job_to_custody_when_its_last_allowed_lease_expires_unsettled() {
    let pool = get_postgres_client().await;
    let scope = scope(
        "expiry-budget",
        QueueOrdering::Standard,
        SHORT,
        2,
        Duration::ZERO,
    );
    let exhausted = push(&pool, &scope, None, "{\"exhausted\":true}").await;
    let queue = JobQueue::new(pool.clone(), scope);

    assert_eq!(vec![exhausted], claim_ids(&queue, 10).await);
    tokio::time::sleep(SHORT * 2).await;
    assert_eq!(vec![exhausted], claim_ids(&queue, 10).await);
    tokio::time::sleep(SHORT * 2).await;
    let healthy = push(&pool, queue.scope(), None, "{\"healthy\":true}").await;

    assert_eq!(
        vec![healthy],
        claim_ids(&queue, 10).await,
        "the claim parks the exhausted job and leases the rest"
    );
    assert_eq!(
        (
            "{\"exhausted\":true}".to_owned(),
            2,
            "RETRY_BUDGET_EXHAUSTED".to_owned(),
            Some("lease_expired".to_owned())
        ),
        custody_row(&pool, exhausted).await
    );
}
