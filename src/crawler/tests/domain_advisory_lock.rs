use crawler::CrawlerDomainId;
use crawler::spider::advisory_lock::{DomainLock, LocalLockManager};
use serial_test::serial;
use std::sync::Arc;
use test_api::*;
use tokio::sync::Barrier;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

#[serial]
#[aura_integration_test(services = [POSTGRES])]
async fn postgres_advisory_lock_allows_only_one_process_owner() {
    let pool = get_postgres_client().await;
    let domain_id = CrawlerDomainId::new();
    let first_manager = Arc::new(LocalLockManager::with_database(pool.clone()));
    let second_manager = Arc::new(LocalLockManager::with_database(pool));
    let barrier = Arc::new(Barrier::new(2));

    let first_barrier = Arc::clone(&barrier);
    let first_manager_for_task = Arc::clone(&first_manager);
    let first = tokio::spawn(async move {
        first_barrier.wait().await;
        DomainLock::try_acquire_global(&first_manager_for_task, domain_id).await
    });
    let second_barrier = Arc::clone(&barrier);
    let second_manager_for_task = Arc::clone(&second_manager);
    let second = tokio::spawn(async move {
        second_barrier.wait().await;
        DomainLock::try_acquire_global(&second_manager_for_task, domain_id).await
    });

    let first = first
        .await
        .expect("first lock task must join")
        .expect("first lock query must succeed");
    let second = second
        .await
        .expect("second lock task must join")
        .expect("second lock query must succeed");

    assert_eq!(first.is_some() as u8 + second.is_some() as u8, 1);
    drop(first);
    drop(second);
}
