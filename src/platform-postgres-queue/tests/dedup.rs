//! Enqueue is insert-or-nothing per scope and dedup key among jobs no worker has claimed yet.

mod support;

use platform_postgres_queue::{
    DedupKey, Disposition, EnqueueOutcome, JobQueue, NewJob, Settlement, enqueue,
};
use support::{DATABASE, claim_one, class, standard};
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_enqueue_a_dedup_key_once_per_scope_while_its_job_is_unclaimed() {
    let pool = get_postgres_client().await;
    let scope = standard("dedup");
    let other_scope = standard("dedup-other");
    let job = |scope, payload: &str| {
        NewJob::new(scope, None, payload)
            .unwrap()
            .with_dedup_key(DedupKey::new("listing-42").unwrap())
    };

    let EnqueueOutcome::Enqueued(first) = enqueue(&pool, &job(&scope, "first")).await.unwrap()
    else {
        panic!("first enqueue must insert");
    };
    assert_eq!(
        EnqueueOutcome::Deduplicated,
        enqueue(&pool, &job(&scope, "duplicate")).await.unwrap()
    );
    assert!(matches!(
        enqueue(&pool, &job(&other_scope, "other scope"))
            .await
            .unwrap(),
        EnqueueOutcome::Enqueued(_)
    ));

    let queue = JobQueue::new(pool.clone(), scope.clone());
    let claimed = claim_one(&queue).await.unwrap();
    assert_eq!((first, "first"), (claimed.job_id(), claimed.payload()));
    let EnqueueOutcome::Enqueued(second) =
        enqueue(&pool, &job(&scope, "after claim")).await.unwrap()
    else {
        panic!("a claimed job no longer deduplicates");
    };

    assert_eq!(
        Settlement::Released,
        queue
            .settle(claimed.lease(), Disposition::Retry(class("retry")))
            .await
            .unwrap(),
        "releasing the claimed job does not collide with its newer duplicate"
    );
    assert_eq!(2, queue.inspect().await.unwrap().depth);
    assert_ne!(first, second);
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_commit_or_roll_back_an_enqueue_with_the_callers_transaction() {
    let pool = get_postgres_client().await;
    let scope = standard("transactional");
    let job = NewJob::new(&scope, None, "{}").unwrap();
    let queue = JobQueue::new(pool.clone(), scope);

    let mut rolled_back = pool.begin().await.unwrap();
    enqueue(&mut *rolled_back, &job).await.unwrap();
    rolled_back.rollback().await.unwrap();
    assert_eq!(None, claim_one(&queue).await);

    let mut committed = pool.begin().await.unwrap();
    let EnqueueOutcome::Enqueued(id) = enqueue(&mut *committed, &job).await.unwrap() else {
        panic!("enqueue must insert");
    };
    assert_eq!(
        None,
        claim_one(&queue).await,
        "uncommitted jobs are invisible"
    );
    committed.commit().await.unwrap();
    assert_eq!(id, claim_one(&queue).await.unwrap().job_id());
}
