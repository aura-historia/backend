use serde_json::{Map, Value};
use std::{
    io,
    sync::{Arc, Mutex, MutexGuard, Once},
};
use tracing::{
    Metadata, Subscriber,
    level_filters::LevelFilter,
    subscriber::{DefaultGuard, Interest, NoSubscriber},
};
use tracing_subscriber::{
    layer::{Layer, SubscriberExt},
    util::SubscriberInitExt,
};

/// Captures `tracing` events on the current thread in the production JSON log shape.
///
/// Use it to prove which structured fields a boundary records and that nothing else, such as an
/// error chain or payload, reaches the log line.
#[derive(Clone, Default)]
pub struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl CapturedLogs {
    /// Captures until the returned guard is dropped. Async tests must run on a current-thread
    /// runtime, which is the `#[tokio::test]` default.
    pub fn start() -> (Self, DefaultGuard) {
        reevaluate_callsites_per_event();
        let logs = Self::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_current_span(true)
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || writer.clone())
            .finish();
        (logs, tracing::subscriber::set_default(subscriber))
    }

    /// Every captured line exactly as it would be written.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.buffer()).into_owned()
    }

    /// The `fields` object of each captured event whose `field` equals `value`, for example its
    /// `message` or its `event` name.
    pub fn events_where(&self, field: &str, value: &str) -> Vec<Map<String, Value>> {
        self.text()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter_map(|line| match line {
                Value::Object(mut line) => match line.remove("fields") {
                    Some(Value::Object(fields)) => Some(fields),
                    _ => None,
                },
                _ => None,
            })
            .filter(|fields| fields.get(field).and_then(Value::as_str) == Some(value))
            .collect()
    }

    fn buffer(&self) -> MutexGuard<'_, Vec<u8>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Tests share callsites across threads. A callsite first reached on a thread without a
/// subscriber would otherwise cache "never" and hide the event from a capture on another thread.
/// A global subscriber that asks for re-evaluation keeps each event's decision with the current
/// thread's subscriber; on its own it records nothing.
fn reevaluate_callsites_per_event() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        // Another test-api fixture may already have installed its global subscriber through
        // `install_global_subscriber`, which re-evaluates callsites the same way.
        let _ =
            tracing::subscriber::set_global_default(NoSubscriber::new().with(ReevaluatePerEvent));
    });
}

/// Installs `subscriber` as the process-wide default, with `log` records forwarded to it, unless
/// one is already installed. Callsites are re-evaluated per event, so a `CapturedLogs` capture
/// still sees events below the levels `subscriber` enables, whichever is installed first.
pub(crate) fn install_global_subscriber(subscriber: impl Subscriber + Send + Sync + 'static) {
    let _ = subscriber.with(ReevaluatePerEvent).try_init();
}

/// Keeps every callsite's interest at "sometimes" and claims no level ceiling, so the wrapped
/// subscriber cannot cache a decision that hides an event from a thread-local capture.
struct ReevaluatePerEvent;

impl<S: Subscriber> Layer<S> for ReevaluatePerEvent {
    fn register_callsite(&self, _: &'static Metadata<'static>) -> Interest {
        Interest::sometimes()
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(LevelFilter::TRACE)
    }
}

impl io::Write for CapturedLogs {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.buffer().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_capture_structured_fields_of_events_with_the_given_message() {
        let (logs, guard) = CapturedLogs::start();
        tracing::warn!(
            stage = "source_read",
            status = Some(503_u16),
            "record retained"
        );
        tracing::info!("unrelated");
        drop(guard);

        let events = logs.events_where("message", "record retained");

        assert_eq!(1, events.len());
        assert_eq!(Some("source_read"), events[0]["stage"].as_str());
        assert_eq!(Some(503), events[0]["status"].as_u64());
    }

    #[test]
    fn should_keep_callsites_reevaluated_below_a_wrapped_subscriber_level() {
        let wrapped = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .finish()
            .with(ReevaluatePerEvent);

        assert!(wrapped.register_callsite(&DEBUG_METADATA).is_sometimes());
        assert_eq!(Some(LevelFilter::TRACE), wrapped.max_level_hint());
        assert!(!wrapped.enabled(&DEBUG_METADATA));
    }

    struct DebugCallsite;

    static DEBUG_CALLSITE: DebugCallsite = DebugCallsite;
    static DEBUG_METADATA: Metadata<'static> = tracing::metadata::Metadata::new(
        "debug event",
        module_path!(),
        tracing::Level::DEBUG,
        None,
        None,
        None,
        tracing::field::FieldSet::new(&[], tracing::callsite::Identifier(&DEBUG_CALLSITE)),
        tracing::metadata::Kind::EVENT,
    );

    impl tracing::Callsite for DebugCallsite {
        fn set_interest(&self, _: Interest) {}

        fn metadata(&self) -> &Metadata<'_> {
            &DEBUG_METADATA
        }
    }
}
