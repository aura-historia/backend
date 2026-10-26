//! Graceful stop on SIGTERM and SIGINT. The operator contract is in
//! [operations](../../../../docs/crawler/operations.md#stopping-the-crawler).

use crate::service::cron::CronStop;
use std::future::Future;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{info, warn};

/// How long work already running may take to finish after the first stop signal.
pub const DEFAULT_SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StopSignal {
    Terminate,
    Interrupt,
}

impl StopSignal {
    fn name(self) -> &'static str {
        match self {
            Self::Terminate => "SIGTERM",
            Self::Interrupt => "SIGINT",
        }
    }

    /// The shell convention for a process ended by a signal: 128 plus the signal number.
    pub(super) fn forced_exit_code(self) -> i32 {
        match self {
            Self::Terminate => 143,
            Self::Interrupt => 130,
        }
    }
}

/// How [`supervise`] ended.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum RuntimeExit {
    /// The review server or the cron loop ended before any stop signal.
    ComponentEnded,
    /// The cron loop finished its running work within the grace period.
    Drained,
    /// The grace period ended with work still running.
    GraceExpired,
    /// A second stop signal arrived during the grace period.
    Forced(StopSignal),
}

/// Runs `review` and `cron` until either ends or a stop signal arrives. On the first signal
/// the review server stops, `cron_stop` stops new work, and `cron` gets `grace_period` to
/// finish the work already running; a second signal ends the wait at once.
pub(super) async fn supervise(
    review: impl Future<Output = ()> + Send + 'static,
    cron: impl Future<Output = ()> + Send + 'static,
    cron_stop: CronStop,
    grace_period: Duration,
    mut signals: mpsc::Receiver<StopSignal>,
) -> RuntimeExit {
    let mut review = tokio::spawn(review);
    let mut cron = tokio::spawn(cron);

    let signal = tokio::select! {
        result = &mut review => {
            result.expect("crawler review server task panicked");
            return RuntimeExit::ComponentEnded;
        }
        result = &mut cron => {
            result.expect("crawler cron task panicked");
            return RuntimeExit::ComponentEnded;
        }
        Some(signal) = signals.recv() => signal,
    };

    info!(
        event = "crawler.shutdown.started",
        signal = signal.name(),
        grace_period_ms = grace_period.as_millis(),
        "Crawler stopping: no new spider or scraper work starts; waiting for running work"
    );
    review.abort();
    cron_stop.request();

    tokio::select! {
        result = &mut cron => {
            result.expect("crawler cron task panicked");
            info!(
                event = "crawler.shutdown.completed",
                signal = signal.name(),
                "Crawler stopped after running work finished"
            );
            RuntimeExit::Drained
        }
        _ = tokio::time::sleep(grace_period) => {
            info!(
                event = "crawler.shutdown.grace_period_expired",
                signal = signal.name(),
                grace_period_ms = grace_period.as_millis(),
                "Crawler stopping with work still running after the grace period"
            );
            RuntimeExit::GraceExpired
        }
        Some(second) = signals.recv() => {
            warn!(
                event = "crawler.shutdown.forced",
                signal = second.name(),
                "Crawler exiting immediately on a second stop signal"
            );
            RuntimeExit::Forced(second)
        }
    }
}

/// Installs the SIGTERM and SIGINT handlers and forwards each signal. From here on neither
/// signal ends the process by itself; [`supervise`] decides.
#[cfg(unix)]
pub(super) fn os_stop_signals() -> mpsc::Receiver<StopSignal> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut terminate =
        signal(SignalKind::terminate()).expect("failed to install the SIGTERM handler");
    let mut interrupt =
        signal(SignalKind::interrupt()).expect("failed to install the SIGINT handler");
    let (sender, receiver) = mpsc::channel(2);
    tokio::spawn(async move {
        loop {
            let signal = tokio::select! {
                Some(()) = terminate.recv() => StopSignal::Terminate,
                Some(()) = interrupt.recv() => StopSignal::Interrupt,
                else => break,
            };
            if sender.send(signal).await.is_err() {
                break;
            }
        }
    });
    receiver
}

/// Ctrl+C is the only stop signal outside Unix.
#[cfg(not(unix))]
pub(super) fn os_stop_signals() -> mpsc::Receiver<StopSignal> {
    let (sender, receiver) = mpsc::channel(2);
    tokio::spawn(async move {
        while tokio::signal::ctrl_c().await.is_ok() {
            if sender.send(StopSignal::Interrupt).await.is_err() {
                break;
            }
        }
    });
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    const GRACE_PERIOD: Duration = Duration::from_secs(30);

    /// A cron loop that notices the stop request and then needs `drain` to finish its work.
    async fn cron_draining_for(stop: CronStop, drain: Duration) {
        while !stop.is_requested() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(drain).await;
    }

    fn signal_channel() -> (mpsc::Sender<StopSignal>, mpsc::Receiver<StopSignal>) {
        mpsc::channel(2)
    }

    #[tokio::test(start_paused = true)]
    async fn should_stop_the_review_server_and_drain_cron_within_the_grace_period() {
        let stop = CronStop::default();
        let review_dropped = Arc::new(AtomicBool::new(false));
        let review_guard = DropFlag(Arc::clone(&review_dropped));
        let (signals, receiver) = signal_channel();
        signals
            .send(StopSignal::Terminate)
            .await
            .expect("queue the stop signal");
        let started = tokio::time::Instant::now();

        let exit = supervise(
            async move {
                let _guard = review_guard;
                std::future::pending::<()>().await;
            },
            cron_draining_for(stop.clone(), Duration::from_secs(5)),
            stop.clone(),
            GRACE_PERIOD,
            receiver,
        )
        .await;

        assert_eq!(exit, RuntimeExit::Drained);
        assert!(stop.is_requested());
        assert!(started.elapsed() < GRACE_PERIOD);
        tokio::task::yield_now().await;
        assert!(review_dropped.load(Ordering::SeqCst));
    }

    #[tokio::test(start_paused = true)]
    async fn should_stop_waiting_when_the_grace_period_expires() {
        let stop = CronStop::default();
        let (signals, receiver) = signal_channel();
        signals
            .send(StopSignal::Interrupt)
            .await
            .expect("queue the stop signal");
        let started = tokio::time::Instant::now();

        let exit = supervise(
            std::future::pending(),
            cron_draining_for(stop.clone(), Duration::from_secs(3600)),
            stop,
            GRACE_PERIOD,
            receiver,
        )
        .await;

        assert_eq!(exit, RuntimeExit::GraceExpired);
        assert!(started.elapsed() >= GRACE_PERIOD);
    }

    #[tokio::test(start_paused = true)]
    async fn should_exit_at_once_on_a_second_signal() {
        let stop = CronStop::default();
        let (signals, receiver) = signal_channel();
        signals
            .send(StopSignal::Interrupt)
            .await
            .expect("queue the first stop signal");
        signals
            .send(StopSignal::Terminate)
            .await
            .expect("queue the second stop signal");
        let started = tokio::time::Instant::now();

        let exit = supervise(
            std::future::pending(),
            cron_draining_for(stop.clone(), Duration::from_secs(3600)),
            stop,
            GRACE_PERIOD,
            receiver,
        )
        .await;

        assert_eq!(exit, RuntimeExit::Forced(StopSignal::Terminate));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn should_run_until_a_component_ends_without_a_signal() {
        let stop = CronStop::default();
        let (_signals, receiver) = signal_channel();

        let exit = supervise(
            std::future::pending(),
            async {},
            stop.clone(),
            GRACE_PERIOD,
            receiver,
        )
        .await;

        assert_eq!(exit, RuntimeExit::ComponentEnded);
        assert!(!stop.is_requested());
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial]
    async fn should_forward_sigterm_instead_of_ending_the_process() {
        let mut signals = os_stop_signals();

        // SAFETY: `kill` on the own process id; the handler above is installed already.
        let sent = unsafe { libc::kill(libc::getpid(), libc::SIGTERM) };

        assert_eq!(sent, 0);
        let signal = tokio::time::timeout(Duration::from_secs(5), signals.recv())
            .await
            .expect("SIGTERM must be forwarded");
        assert_eq!(signal, Some(StopSignal::Terminate));
    }

    struct DropFlag(Arc<AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
}
