//! Operator functions: inspection and redrive of failure custody.

mod support;

use platform_postgres_queue::{
    CustodyReason, DedupKey, Disposition, EnqueueOutcome, JobId, JobQueue, NewJob, QueueOrdering,
    RedriveReport, RedriveSelection, Settlement, enqueue,
};
use std::time::Duration;
use support::{DATABASE, LONG, ack, claim_one, class, push, scope, standard};
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use uuid::Uuid;

async fn exhaust(queue: &JobQueue) -> JobId {
    let job = claim_one(queue).await.unwrap();
    assert_eq!(
        Settlement::MovedToCustody(CustodyReason::RetryBudgetExhausted),
        queue
            .settle(job.lease(), Disposition::Retry(class("dependency_failed")))
            .await
            .unwrap()
    );
    job.job_id()
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_redrive_custody_with_its_identity_and_group_order() {
    let pool = get_postgres_client().await;
    let scope = scope(
        "redrive-order",
        QueueOrdering::Fifo,
        LONG,
        1,
        Duration::ZERO,
    );
    let mut enqueued = Vec::new();
    for payload in ["first", "second", "third"] {
        enqueued.push(push(&pool, &scope, Some("g"), payload).await);
    }
    let queue = JobQueue::new(pool.clone(), scope);
    for expected in &enqueued {
        assert_eq!(*expected, exhaust(&queue).await);
    }
    let later = push(&pool, queue.scope(), Some("g"), "later").await;
    let leased_later = claim_one(&queue).await.unwrap();
    assert_eq!(later, leased_later.job_id());

    let report = queue.redrive(&RedriveSelection::All).await.unwrap();

    assert_eq!(
        RedriveReport {
            redriven: enqueued.clone(),
            ..RedriveReport::default()
        },
        report
    );
    assert_eq!(
        None,
        claim_one(&queue).await,
        "redriven jobs wait while their group is leased"
    );
    ack(&queue, &leased_later).await;
    for (id, payload) in enqueued.iter().zip(["first", "second", "third"]) {
        let job = claim_one(&queue).await.unwrap();
        assert_eq!(
            (*id, payload, 1),
            (job.job_id(), job.payload(), job.attempt())
        );
        ack(&queue, &job).await;
    }
    let stats = queue.inspect().await.unwrap();
    assert_eq!((0, 0), (stats.depth, stats.in_custody));
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_redrive_only_selected_jobs_and_keep_conflicting_ones_in_custody() {
    let pool = get_postgres_client().await;
    let scope = scope(
        "redrive-select",
        QueueOrdering::Standard,
        LONG,
        1,
        Duration::ZERO,
    );
    let keyed = |payload: &str| {
        NewJob::new(&scope, None, payload)
            .unwrap()
            .with_dedup_key(DedupKey::new("same-key").unwrap())
    };
    let queue = JobQueue::new(pool.clone(), scope.clone());
    let EnqueueOutcome::Enqueued(conflicting) = enqueue(&pool, &keyed("old")).await.unwrap() else {
        panic!("enqueue must insert");
    };
    assert_eq!(conflicting, exhaust(&queue).await);
    let untouched = push(&pool, &scope, None, "untouched").await;
    assert_eq!(untouched, exhaust(&queue).await);
    let selected = push(&pool, &scope, None, "selected").await;
    assert_eq!(selected, exhaust(&queue).await);
    assert!(matches!(
        enqueue(&pool, &keyed("new")).await.unwrap(),
        EnqueueOutcome::Enqueued(_)
    ));
    let unknown = JobId::from_uuid(Uuid::new_v4());

    let report = queue
        .redrive(&RedriveSelection::Jobs(vec![
            selected,
            conflicting,
            unknown,
        ]))
        .await
        .unwrap();

    assert_eq!(
        RedriveReport {
            redriven: vec![selected],
            conflicting: vec![conflicting],
            not_in_custody: vec![unknown],
        },
        report
    );
    assert_eq!(2, queue.inspect().await.unwrap().in_custody);
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_report_depth_leases_oldest_waiting_age_and_custody_per_scope() {
    let pool = get_postgres_client().await;
    let scope = scope("inspect", QueueOrdering::Standard, LONG, 1, Duration::ZERO);
    let other = standard("inspect-other");
    let queue = JobQueue::new(pool.clone(), scope.clone());
    push(&pool, &scope, None, "custody").await;
    exhaust(&queue).await;
    push(&pool, &scope, None, "leased").await;
    let _leased = claim_one(&queue).await.unwrap();
    push(&pool, &other, None, "other scope").await;
    let before = queue.inspect().await.unwrap();
    assert_eq!(
        (0, 1, None, 1),
        (
            before.depth,
            before.leased,
            before.oldest_waiting_age,
            before.in_custody
        )
    );

    push(&pool, &scope, None, "waiting").await;
    push(&pool, &scope, None, "waiting").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let stats = queue.inspect().await.unwrap();

    assert_eq!((2, 1, 1), (stats.depth, stats.leased, stats.in_custody));
    let age = stats.oldest_waiting_age.unwrap();
    assert!(
        age >= Duration::from_millis(200) && age < Duration::from_secs(60),
        "{age:?}"
    );
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_overview_every_scopes_oldest_waiting_job_and_all_custody() {
    let pool = get_postgres_client().await;
    let busy = scope(
        "overview-busy",
        QueueOrdering::Standard,
        LONG,
        1,
        Duration::ZERO,
    );
    let leased_only = standard("overview-leased");
    let busy_queue = JobQueue::new(pool.clone(), busy.clone());
    push(&pool, &busy, None, "custody").await;
    exhaust(&busy_queue).await;
    push(&pool, &busy, None, "waiting").await;
    push(&pool, &leased_only, None, "leased").await;
    let _leased = claim_one(&JobQueue::new(pool.clone(), leased_only.clone()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let overview = platform_postgres_queue::overview(&mut pool.acquire().await.unwrap())
        .await
        .unwrap();

    assert_eq!(1, overview.in_custody);
    assert_eq!(
        vec![busy.name().clone()],
        overview
            .oldest_waiting
            .iter()
            .map(|(scope, _)| scope.clone())
            .collect::<Vec<_>>(),
        "a scope whose only job is leased has no waiting job"
    );
    let age = overview.oldest_waiting[0].1;
    assert!(
        age >= Duration::from_millis(200) && age < Duration::from_secs(60),
        "{age:?}"
    );
}
