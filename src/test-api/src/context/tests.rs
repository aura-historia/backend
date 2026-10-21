use super::*;
use std::sync::atomic::AtomicUsize;

#[tokio::test]
async fn should_keep_context_explicit_across_tasks_and_cleanup_before_reuse() {
    let session = TestContext::acquire_explicit("cancellation", Isolation::Isolated)
        .await
        .unwrap();
    let context = session.context().clone();
    let stopped = Arc::new(AtomicBool::new(false));
    let finished = stopped.clone();
    context
        .on_shutdown(async move {
            finished.store(true, Ordering::Release);
            Ok(())
        })
        .unwrap();
    let (started, ready) = tokio::sync::oneshot::channel();
    let expected = context.test_id();
    context
        .spawn(async move {
            assert_eq!(expected, TestContext::current().unwrap().test_id());
            let _ = started.send(());
            std::future::pending::<()>().await;
        })
        .unwrap();
    ready.await.unwrap();
    drop(session);
    let next = TestContext::reserve(
        "after cancellation",
        Isolation::Isolated,
        lease::Runner {
            invocation: uuid::Uuid::new_v4().simple().to_string(),
            slot: Some(context.slot()),
        },
    )
    .await
    .unwrap();
    assert!(stopped.load(Ordering::Acquire));
    assert!(
        context.product_listings_index().is_err(),
        "expired contexts cannot register more resources"
    );
    next.finish().await.unwrap();
}

#[tokio::test]
async fn should_report_cleanup_failure_and_recover_the_slot_before_reuse() {
    let session = TestContext::acquire_explicit("cleanup failure", Isolation::Isolated)
        .await
        .unwrap();
    let slot = session.context().slot();
    let stopped = Arc::new(AtomicBool::new(false));
    let finished = stopped.clone();
    session
        .context()
        .on_shutdown(async move {
            finished.store(true, Ordering::Release);
            Ok(())
        })
        .unwrap();
    session
        .context()
        .on_shutdown(async { Err(failure("cleanup canary")) })
        .unwrap();
    let error = session.finish().await.unwrap_err();
    assert!(error.to_string().contains("cleanup canary"));
    assert!(
        stopped.load(Ordering::Acquire),
        "one cleanup failure must not skip other shutdowns"
    );
    let next = TestContext::reserve(
        "cleanup recovery",
        Isolation::Isolated,
        lease::Runner {
            invocation: uuid::Uuid::new_v4().simple().to_string(),
            slot: Some(slot),
        },
    )
    .await
    .unwrap();
    next.finish().await.unwrap();
}

#[tokio::test]
async fn should_bound_non_nextest_execution_through_the_same_reservation() {
    let running = Arc::new(AtomicUsize::new(0));
    let mut tasks = vec![];
    for _ in 0..4 {
        let running = running.clone();
        tasks.push(tokio::spawn(async move {
            let session = TestContext::acquire_explicit("bounded", Isolation::Exclusive)
                .await
                .unwrap();
            assert_eq!(0, running.fetch_add(1, Ordering::SeqCst));
            tokio::task::yield_now().await;
            assert_eq!(1, running.fetch_sub(1, Ordering::SeqCst));
            session.finish().await.unwrap();
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
}

#[test]
fn should_reject_recovery_metadata_that_belongs_to_another_run_or_slot() {
    let manifest = Manifest {
        version: 1,
        run: "0".repeat(32),
        invocation: uuid::Uuid::new_v4().simple().to_string(),
        test: "1".repeat(32),
        name: "ownership".into(),
        slot: 0,
        resources: vec![Resource {
            kind: Kind::Queue,
            name: "foreign-queue".into(),
        }],
    };
    assert!(manifest.validate(&manifest.run, 0).is_err());
    assert!(manifest.validate(&"2".repeat(32), 0).is_err());
    assert!(manifest.validate(&manifest.run, 1).is_err());
}

#[cfg(feature = "aura-historia-api")]
#[tokio::test]
async fn should_own_typed_api_state_and_share_the_same_arc_across_threads() {
    let session = TestContext::acquire_explicit("API state", Isolation::Isolated)
        .await
        .unwrap();
    let context = session.context().clone();
    assert!(context.api_state::<Arc<AtomicUsize>>().is_err());
    let state = Arc::new(AtomicUsize::new(17));
    context.insert_api_state(state.clone()).unwrap();
    assert!(context.insert_api_state(state.clone()).is_err());
    let thread_context = context.clone();
    let threaded =
        std::thread::spawn(move || thread_context.api_state::<Arc<AtomicUsize>>().unwrap())
            .join()
            .unwrap();
    assert!(Arc::ptr_eq(&state, &threaded));
    assert_eq!(17, threaded.load(Ordering::SeqCst));
    let weak = Arc::downgrade(&state);
    drop(threaded);
    drop(state);
    session.finish().await.unwrap();
    assert!(
        weak.upgrade().is_none(),
        "context must release API fake state after server shutdown"
    );
    assert!(context.api_state::<Arc<AtomicUsize>>().is_err());
    let next = TestContext::acquire_explicit("another API state", Isolation::Isolated)
        .await
        .unwrap();
    assert!(next.context().api_state::<Arc<AtomicUsize>>().is_err());
    next.finish().await.unwrap();
}

#[tokio::test]
async fn should_continue_shutdown_and_recover_when_a_cleanup_future_panics() {
    let session = TestContext::acquire_explicit("panicking cleanup", Isolation::Isolated)
        .await
        .unwrap();
    let slot = session.context().slot();
    let stopped = Arc::new(AtomicBool::new(false));
    let finished = stopped.clone();
    session
        .context()
        .on_shutdown(async move {
            finished.store(true, Ordering::Release);
            Ok(())
        })
        .unwrap();
    session
        .context()
        .on_shutdown(async {
            panic!("cleanup future canary");
        })
        .unwrap();
    assert!(
        session
            .finish()
            .await
            .unwrap_err()
            .to_string()
            .contains("shutdown panicked")
    );
    assert!(stopped.load(Ordering::Acquire));
    let next = TestContext::reserve(
        "after cleanup panic",
        Isolation::Isolated,
        lease::Runner {
            invocation: uuid::Uuid::new_v4().simple().to_string(),
            slot: Some(slot),
        },
    )
    .await
    .unwrap();
    next.finish().await.unwrap();
}

#[tokio::test]
async fn should_complete_concurrent_metadata_writes_before_releasing_the_slot() {
    let session = TestContext::acquire_explicit("concurrent metadata", Isolation::Isolated)
        .await
        .unwrap();
    let context = session.context().clone();
    let path = context.0.path.clone();
    let mut writers = Vec::new();
    for _ in 0..8 {
        let context = context.clone();
        writers.push(tokio::spawn(
            async move { context.persist_resources().await },
        ));
    }
    session.finish().await.unwrap();
    for writer in writers {
        // Teardown may close the context before a queued writer acquires the mutex.
        let _ = writer.await.unwrap();
    }
    assert!(!tokio::fs::try_exists(path).await.unwrap());
    assert!(context.persist_resources().await.is_err());
}
