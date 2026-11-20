//! Idle workers wake on a committed enqueue of their scope and otherwise poll.

mod support;

use platform_postgres_queue::{JobWakeups, NewJob, Wakeup, enqueue};
use std::time::Duration;
use support::{DATABASE, standard};
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};

const POLL: Duration = Duration::from_millis(200);

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_wake_listeners_of_the_scope_when_an_enqueue_commits() {
    let pool = get_postgres_client().await;
    let watched = standard("watched");
    let unwatched = standard("unwatched");
    let mut wakeups = JobWakeups::listen(&pool, [watched.name()]).await.unwrap();

    enqueue(&pool, &NewJob::new(&unwatched, None, "{}").unwrap())
        .await
        .unwrap();
    assert_eq!(Wakeup::Poll, wakeups.wait(POLL).await.unwrap());

    let mut transaction = pool.begin().await.unwrap();
    for _ in 0..3 {
        enqueue(
            &mut *transaction,
            &NewJob::new(&watched, None, "{}").unwrap(),
        )
        .await
        .unwrap();
    }
    assert_eq!(
        Wakeup::Poll,
        wakeups.wait(POLL).await.unwrap(),
        "nothing is announced before commit"
    );
    transaction.commit().await.unwrap();

    assert_eq!(
        Wakeup::Enqueued(watched.name().clone()),
        wakeups.wait(Duration::from_secs(10)).await.unwrap()
    );
    assert_eq!(
        Wakeup::Poll,
        wakeups.wait(POLL).await.unwrap(),
        "one transaction announces its scope once"
    );
}
