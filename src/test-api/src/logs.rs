use serde_json::{Map, Value};
use std::{
    io,
    sync::{Arc, Mutex, MutexGuard, Once},
};
use tracing::{
    Event, Metadata, Subscriber,
    span::{Attributes, Id, Record},
    subscriber::{DefaultGuard, Interest},
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
        // If another global subscriber is already installed, callsites at the levels it enables
        // keep a non-"never" interest; captures below those levels can then still be missed.
        let _ = tracing::subscriber::set_global_default(ReevaluatePerEvent);
    });
}

struct ReevaluatePerEvent;

impl Subscriber for ReevaluatePerEvent {
    fn register_callsite(&self, _: &'static Metadata<'static>) -> Interest {
        Interest::sometimes()
    }

    fn enabled(&self, _: &Metadata<'_>) -> bool {
        false
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, _: &Event<'_>) {}

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
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
}
