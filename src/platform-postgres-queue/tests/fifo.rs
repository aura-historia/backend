//! FIFO scopes: head-of-line blocking within a group, isolation between groups, and the
//! park disposition that releases a group from a job that can never succeed.

mod support;

use futures::future::{join, join_all};
use platform_postgres_queue::{
    CustodyReason, Disposition, EnqueueOutcome, GroupKey, JobId, JobQueue, NewJob, Settlement,
    enqueue,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
    time::{Duration, Instant},
};
use support::{DATABASE, ack, claim_ids, claim_one, class, custody_row, fifo, push, sorted};
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_release_only_the_oldest_job_of_a_group_and_only_while_none_is_leased() {
    let pool = get_postgres_client().await;
    let scope = fifo("fifo-head");
    let a1 = push(&pool, &scope, Some("a"), "a1").await;
    let a2 = push(&pool, &scope, Some("a"), "a2").await;
    let b1 = push(&pool, &scope, Some("b"), "b1").await;
    let b2 = push(&pool, &scope, Some("b"), "b2").await;
    let queue = JobQueue::new(pool, scope);

    let heads = queue.claim(10).await.unwrap();
    assert_eq!(
        sorted(vec![a1, b1]),
        sorted(heads.iter().map(|job| job.job_id()).collect())
    );
    assert!(
        claim_ids(&queue, 10).await.is_empty(),
        "leased heads block their groups"
    );

    let (head_a, head_b) = if heads[0].job_id() == a1 {
        (&heads[0], &heads[1])
    } else {
        (&heads[1], &heads[0])
    };
    assert_eq!(
        Settlement::Released,
        queue
            .settle(
                head_a.lease(),
                Disposition::Retry(class("dependency_unavailable"))
            )
            .await
            .unwrap()
    );
    ack(&queue, head_b).await;

    let next = queue.claim(10).await.unwrap();
    assert_eq!(
        sorted(vec![a1, b2]),
        sorted(next.iter().map(|job| job.job_id()).collect()),
        "a retried head stays ahead of its group while the other group moves on"
    );
    for job in &next {
        ack(&queue, job).await;
    }
    assert_eq!(vec![a2], claim_ids(&queue, 10).await);
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_keep_each_group_in_enqueue_order_while_another_group_is_stuck() {
    let pool = get_postgres_client().await;
    let scope = fifo("fifo-isolation");
    let stuck = push(&pool, &scope, Some("stuck"), "stuck").await;
    let mut flowing = Vec::new();
    for index in 0..5 {
        flowing.push(push(&pool, &scope, Some("flowing"), &format!("f{index}")).await);
    }
    let queue = JobQueue::new(pool, scope);

    let mut order = Vec::new();
    loop {
        let jobs = queue.claim(10).await.unwrap();
        if jobs.is_empty() {
            break;
        }
        for job in jobs {
            if job.job_id() == stuck {
                // Never settled within the test: its lease holds the group.
                continue;
            }
            order.push(job.job_id());
            ack(&queue, &job).await;
        }
    }

    assert_eq!(flowing, order);
    assert_eq!(1, queue.inspect().await.unwrap().leased);
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_park_a_job_that_can_never_succeed_on_its_first_redelivery() {
    let pool = get_postgres_client().await;
    let scope = fifo("fifo-park");
    let poison = push(&pool, &scope, Some("g"), "{\"poison\":true}").await;
    let successor = push(&pool, &scope, Some("g"), "{\"next\":true}").await;
    let queue = JobQueue::new(pool.clone(), scope);

    let first = claim_one(&queue).await.unwrap();
    assert_eq!((poison, 1), (first.job_id(), first.attempt()));
    assert_eq!(
        Settlement::Released,
        queue
            .settle(first.lease(), Disposition::Park(class("invalid_job")))
            .await
            .unwrap(),
        "a first delivery is retained, not parked"
    );

    let redelivery = claim_one(&queue).await.unwrap();
    assert_eq!((poison, 2), (redelivery.job_id(), redelivery.attempt()));
    assert_eq!(
        Settlement::MovedToCustody(CustodyReason::Parked),
        queue
            .settle(redelivery.lease(), Disposition::Park(class("invalid_job")))
            .await
            .unwrap()
    );

    let next = claim_one(&queue).await.unwrap();
    assert_eq!(successor, next.job_id(), "parking releases the group");
    assert_eq!(
        (
            "{\"poison\":true}".to_owned(),
            2,
            "PARKED".to_owned(),
            Some("invalid_job".to_owned())
        ),
        custody_row(&pool, poison).await
    );
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_never_lease_two_jobs_of_a_group_to_concurrent_claimers() {
    let pool = get_postgres_client().await;
    let scope = fifo("fifo-concurrent");
    let mut expected: HashMap<String, Vec<JobId>> = HashMap::new();
    for index in 0..240 {
        let group = format!("group-{}", index % 12);
        let id = push(&pool, &scope, Some(&group), &format!("{index}")).await;
        expected.entry(group).or_default().push(id);
    }
    let queue = JobQueue::new(pool, scope);
    let active = Mutex::new(HashSet::new());
    let completed: Mutex<HashMap<String, Vec<JobId>>> = Mutex::new(HashMap::new());

    let workers = (0..8).map(|_| async {
        let mut idle_rounds = 0;
        while idle_rounds < 3 {
            let jobs = queue.claim(4).await.unwrap();
            if jobs.is_empty() {
                idle_rounds += 1;
                tokio::time::sleep(Duration::from_millis(20)).await;
                continue;
            }
            idle_rounds = 0;
            for job in &jobs {
                let group = job.group_key().unwrap().as_str().to_owned();
                assert!(
                    active.lock().unwrap().insert(group.clone()),
                    "two jobs of {group} were leased at once"
                );
            }
            tokio::task::yield_now().await;
            for job in &jobs {
                let group = job.group_key().unwrap().as_str().to_owned();
                completed
                    .lock()
                    .unwrap()
                    .entry(group.clone())
                    .or_default()
                    .push(job.job_id());
                active.lock().unwrap().remove(&group);
                ack(&queue, job).await;
            }
        }
    });
    join_all(workers).await;

    assert_eq!(expected, completed.into_inner().unwrap());
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_make_concurrent_enqueues_of_a_group_visible_in_sequence_order() {
    let pool = get_postgres_client().await;
    let scope = fifo("fifo-enqueue-order");
    let job =
        |payload: &str| NewJob::new(&scope, Some(GroupKey::new("g").unwrap()), payload).unwrap();
    let mut first = pool.begin().await.unwrap();
    let EnqueueOutcome::Enqueued(first_id) = enqueue(&mut *first, &job("first")).await.unwrap()
    else {
        panic!("enqueue must insert");
    };

    let second = async {
        let mut transaction = pool.begin().await.unwrap();
        let outcome = enqueue(&mut *transaction, &job("second")).await.unwrap();
        transaction.commit().await.unwrap();
        (outcome, Instant::now())
    };
    let commit_first = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let committing = Instant::now();
        first.commit().await.unwrap();
        committing
    };
    let ((second_outcome, second_committed), first_committing) = join(second, commit_first).await;

    assert!(
        second_committed >= first_committing,
        "the later enqueue of a group waits for the earlier one to commit"
    );
    let EnqueueOutcome::Enqueued(second_id) = second_outcome else {
        panic!("enqueue must insert");
    };
    let queue = JobQueue::new(pool, scope.clone());
    let head = claim_one(&queue).await.unwrap();
    assert_eq!(first_id, head.job_id());
    ack(&queue, &head).await;
    assert_eq!(second_id, claim_one(&queue).await.unwrap().job_id());
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_park_a_first_delivery_whose_retry_budget_is_spent() {
    let pool = get_postgres_client().await;
    let scope = support::scope(
        "fifo-park-budget",
        platform_postgres_queue::QueueOrdering::Fifo,
        support::LONG,
        1,
        Duration::ZERO,
    );
    let poison = push(&pool, &scope, Some("g"), "{}").await;
    let queue = JobQueue::new(pool.clone(), scope);

    let job = claim_one(&queue).await.unwrap();

    assert_eq!(
        Settlement::MovedToCustody(CustodyReason::Parked),
        queue
            .settle(job.lease(), Disposition::Park(class("invalid_job")))
            .await
            .unwrap()
    );
    assert_eq!("PARKED", custody_row(&pool, poison).await.2);
}
