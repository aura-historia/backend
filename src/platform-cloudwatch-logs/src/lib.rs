//! A dedicated CloudWatch Logs destination for the events of one `tracing` target, independent of
//! the process's log level and of its general log shipping.
//!
//! [`channel`] returns the [`TargetRecords`] whose layer formats each event of the target as one
//! JSON line (the shape of the process's own JSON log) into a bounded queue, and the
//! [`RecordReceiver`] a [`Shipper`] drains into its log groups with `PutLogEvents`. The layer
//! never blocks or fails the code that logs: a full queue drops the record and counts it. A group
//! that refuses every delivery for [`GIVE_UP_AFTER`] spills what it holds, line by line, to the
//! process's stderr, so the records survive in the container log instead of holding back every
//! later one. Delivery is best-effort; the shipper reports what it drops without its content.
use application::error::BoxError;
use aws_sdk_cloudwatchlogs::types::InputLogEvent;
use platform_aws::AwsSdkFailure;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot};
use tracing::{Level, Subscriber};
use tracing_subscriber::{Layer, filter::Targets, fmt::MakeWriter, registry::LookupSpan};

/// A group whose deliveries all fail for this long spills its pending records to stderr, so a
/// batch the service will never accept cannot hold back every later one.
pub const GIVE_UP_AFTER: Duration = Duration::from_secs(15 * 60);
/// While a group's deliveries fail, the failure is logged at most this often.
const FAILURE_LOG_INTERVAL: Duration = Duration::from_secs(60);
/// `PutLogEvents` accepts at most 10,000 events and 1,048,576 bytes, counting 26 bytes per event.
const MAX_BATCH_EVENTS: usize = 1_000;
const MAX_BATCH_BYTES: usize = 900_000;
const EVENT_OVERHEAD_BYTES: usize = 26;
/// Records a group may hold while its deliveries fail; older ones are dropped first.
const MAX_PENDING_PER_GROUP: usize = 10_000;

/// One formatted event and the time it was logged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord {
    pub timestamp_ms: i64,
    pub message: String,
}

/// The layer side of [`channel`]: formats the target's events into the queue.
#[derive(Clone)]
pub struct TargetRecords {
    target: &'static str,
    writer: RecordWriter,
}

/// The shipper side of [`channel`].
pub struct RecordReceiver {
    records: mpsc::Receiver<LogRecord>,
    dropped: Arc<AtomicU64>,
}

/// A queue of at most `capacity` records for the events of `target` at INFO or above.
pub fn channel(target: &'static str, capacity: usize) -> (TargetRecords, RecordReceiver) {
    let (sender, records) = mpsc::channel(capacity.max(1));
    let dropped = Arc::new(AtomicU64::new(0));
    (
        TargetRecords {
            target,
            writer: RecordWriter {
                sender,
                dropped: Arc::clone(&dropped),
            },
        },
        RecordReceiver { records, dropped },
    )
}

impl TargetRecords {
    pub fn target(&self) -> &'static str {
        self.target
    }

    /// The layer that queues the target's events whatever the subscriber's other filters allow;
    /// it sees no other target and no span.
    pub fn layer<S>(&self) -> impl Layer<S> + Send + Sync + 'static
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        tracing_subscriber::fmt::layer()
            .json()
            .with_current_span(false)
            .with_span_list(false)
            .with_ansi(false)
            .without_time()
            .with_writer(self.writer.clone())
            .with_filter(Targets::new().with_target(self.target, Level::INFO))
    }
}

#[derive(Clone)]
struct RecordWriter {
    sender: mpsc::Sender<LogRecord>,
    dropped: Arc<AtomicU64>,
}

impl<'a> MakeWriter<'a> for RecordWriter {
    type Writer = LineWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LineWriter {
            line: Vec::new(),
            owner: self.clone(),
        }
    }
}

/// Collects one formatted event and queues it when the formatter drops it.
struct LineWriter {
    line: Vec<u8>,
    owner: RecordWriter,
}

impl std::io::Write for LineWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.line.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for LineWriter {
    fn drop(&mut self) {
        let message = String::from_utf8_lossy(&self.line).trim_end().to_owned();
        if message.is_empty() {
            return;
        }
        let record = LogRecord {
            timestamp_ms: now_ms(),
            message,
        };
        // Never block the logging thread; the shipper reports what the full queue dropped.
        if self.owner.sender.try_send(record).is_err() {
            self.owner.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// Where the shipper delivers records.
#[async_trait::async_trait]
pub trait LogEventSink: Send + Sync + 'static {
    /// Creates `stream` in `group`; a stream that already exists is success.
    async fn create_stream(&self, group: &str, stream: &str) -> Result<(), BoxError>;
    /// Appends `records`, in timestamp order, to `stream` in `group`.
    async fn put(&self, group: &str, stream: &str, records: &[LogRecord]) -> Result<(), BoxError>;
}

pub struct CloudWatchLogsSink {
    client: aws_sdk_cloudwatchlogs::Client,
}

impl CloudWatchLogsSink {
    pub fn new(client: aws_sdk_cloudwatchlogs::Client) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl LogEventSink for CloudWatchLogsSink {
    async fn create_stream(&self, group: &str, stream: &str) -> Result<(), BoxError> {
        match self
            .client
            .create_log_stream()
            .log_group_name(group)
            .log_stream_name(stream)
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(error)
                if error
                    .as_service_error()
                    .is_some_and(|error| error.is_resource_already_exists_exception()) =>
            {
                Ok(())
            }
            Err(error) => Err(sdk_failure("CreateLogStream", &error)),
        }
    }

    async fn put(&self, group: &str, stream: &str, records: &[LogRecord]) -> Result<(), BoxError> {
        let events = records
            .iter()
            .map(|record| {
                InputLogEvent::builder()
                    .timestamp(record.timestamp_ms)
                    .message(&record.message)
                    .build()
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.client
            .put_log_events()
            .log_group_name(group)
            .log_stream_name(stream)
            .set_log_events(Some(events))
            .send()
            .await
            .map(|_| ())
            .map_err(|error| sdk_failure("PutLogEvents", &error))
    }
}

/// Only the operation and the failure class: the SDK error can echo the request.
fn sdk_failure<E>(
    operation: &'static str,
    error: &aws_sdk_cloudwatchlogs::error::SdkError<E>,
) -> BoxError
where
    E: aws_sdk_cloudwatchlogs::error::ProvideErrorMetadata,
{
    let failure = AwsSdkFailure::classify(error);
    format!(
        "CloudWatch Logs {operation} failed: {} (HTTP {:?})",
        failure.kind().diagnostic_category(),
        failure.http_status()
    )
    .into()
}

pub struct ShipperConfig {
    /// The destination groups; `route` picks one by index for each record.
    pub log_groups: Vec<String>,
    /// The stream this process writes to in every group.
    pub stream: String,
    /// The index of a record's group, given its JSON line; an index past the end selects the
    /// first group.
    pub route: fn(&serde_json::Value) -> usize,
    /// How long a record may wait before it is delivered.
    pub flush_interval: Duration,
}

/// Delivers the received records until [`Shipper::close`].
pub struct Shipper {
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

impl Shipper {
    /// Creates this process's stream in every group, so a wrong group name, a missing grant or
    /// missing credentials fail here instead of on the first record, then starts delivering.
    pub async fn start(
        sink: impl LogEventSink,
        receiver: RecordReceiver,
        config: ShipperConfig,
    ) -> Result<Self, BoxError> {
        if config.log_groups.is_empty() {
            return Err("a dedicated CloudWatch target needs at least one log group".into());
        }
        for group in &config.log_groups {
            sink.create_stream(group, &config.stream).await?;
        }
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(ship(sink, receiver, config, stopped));
        Ok(Self { stop, task })
    }

    /// Delivers what is queued, then stops. Gives up after `timeout`.
    pub async fn close(self, timeout: Duration) {
        let _ = self.stop.send(());
        if tokio::time::timeout(timeout, self.task).await.is_err() {
            tracing::warn!(
                event = "cloudwatch_logs.close_timed_out",
                "Dedicated CloudWatch log records were not delivered before shutdown"
            );
        }
    }
}

struct Destination {
    group: String,
    pending: Vec<LogRecord>,
    /// Oldest records dropped at [`MAX_PENDING_PER_GROUP`] since the last report.
    dropped_at_limit: usize,
    failing_since: Option<tokio::time::Instant>,
    failure_logged_at: Option<tokio::time::Instant>,
}

impl Destination {
    fn new(group: String) -> Self {
        Self {
            group,
            pending: Vec::new(),
            dropped_at_limit: 0,
            failing_since: None,
            failure_logged_at: None,
        }
    }
}

async fn ship(
    sink: impl LogEventSink,
    mut receiver: RecordReceiver,
    config: ShipperConfig,
    mut stopped: oneshot::Receiver<()>,
) {
    let mut destinations = config
        .log_groups
        .into_iter()
        .map(Destination::new)
        .collect::<Vec<_>>();
    let mut interval = tokio::time::interval(config.flush_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            record = receiver.records.recv() => {
                let Some(record) = record else { break };
                accept(&mut destinations, config.route, record);
            }
            _ = interval.tick() => {
                report_dropped(&receiver);
                for destination in &mut destinations {
                    deliver(&sink, &config.stream, destination).await;
                }
            }
            _ = &mut stopped => break,
        }
    }
    while let Ok(record) = receiver.records.try_recv() {
        accept(&mut destinations, config.route, record);
    }
    report_dropped(&receiver);
    for destination in &mut destinations {
        deliver(&sink, &config.stream, destination).await;
    }
}

fn accept(
    destinations: &mut [Destination],
    route: fn(&serde_json::Value) -> usize,
    record: LogRecord,
) {
    let index = serde_json::from_str(&record.message)
        .map(|line| route(&line))
        .unwrap_or(0);
    let index = if index < destinations.len() { index } else { 0 };
    let destination = &mut destinations[index];
    destination.pending.push(record);
    if destination.pending.len() > MAX_PENDING_PER_GROUP {
        // The oldest record is spilled like a failed batch, not lost.
        spill(&destination.pending.remove(0));
        destination.dropped_at_limit += 1;
    }
}

fn report_dropped(receiver: &RecordReceiver) {
    let dropped = receiver.dropped.swap(0, Ordering::Relaxed);
    if dropped > 0 {
        tracing::warn!(
            event = "cloudwatch_logs.records_dropped",
            dropped,
            reason = "queue_full",
            "Dropped dedicated CloudWatch log records"
        );
    }
}

/// Delivers the destination's pending records in timestamp order, in batches within the
/// `PutLogEvents` limits. A failure keeps the rest for the next attempt.
async fn deliver(sink: &impl LogEventSink, stream: &str, destination: &mut Destination) {
    if destination.dropped_at_limit > 0 {
        tracing::warn!(
            event = "cloudwatch_logs.records_spilled",
            log_group = %destination.group,
            spilled = std::mem::take(&mut destination.dropped_at_limit),
            reason = "pending_limit",
            "Spilled dedicated CloudWatch log records to stderr"
        );
    }
    if destination.pending.is_empty() {
        return;
    }
    destination
        .pending
        .sort_by_key(|record| record.timestamp_ms);
    while !destination.pending.is_empty() {
        let size = batch_len(&destination.pending);
        if let Err(error) = sink
            .put(&destination.group, stream, &destination.pending[..size])
            .await
        {
            return failed(destination, &error);
        }
        destination.pending.drain(..size);
        destination.failing_since = None;
        destination.failure_logged_at = None;
    }
}

/// Keeps an undeliverable record in the container log, as its own JSON line on stderr.
fn spill(record: &LogRecord) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr().lock(), "{}", record.message);
}

fn batch_len(records: &[LogRecord]) -> usize {
    let mut bytes = 0;
    records
        .iter()
        .take(MAX_BATCH_EVENTS)
        .take_while(|record| {
            bytes += record.message.len() + EVENT_OVERHEAD_BYTES;
            bytes <= MAX_BATCH_BYTES
        })
        .count()
        .max(1)
}

fn failed(destination: &mut Destination, error: &BoxError) {
    let now = tokio::time::Instant::now();
    let failing_since = *destination.failing_since.get_or_insert(now);
    if now.duration_since(failing_since) >= GIVE_UP_AFTER {
        tracing::error!(
            event = "cloudwatch_logs.records_spilled",
            log_group = %destination.group,
            spilled = destination.pending.len(),
            reason = "delivery_failed",
            error = %error,
            "Spilled dedicated CloudWatch log records to stderr after their deliveries failed for too long"
        );
        for record in destination.pending.drain(..) {
            spill(&record);
        }
        destination.failing_since = None;
        destination.failure_logged_at = None;
        return;
    }
    if destination
        .failure_logged_at
        .is_none_or(|logged| now.duration_since(logged) >= FAILURE_LOG_INTERVAL)
    {
        destination.failure_logged_at = Some(now);
        tracing::warn!(
            event = "cloudwatch_logs.delivery_failed",
            log_group = %destination.group,
            pending = destination.pending.len(),
            error = %error,
            "Dedicated CloudWatch log delivery failed; retrying"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tracing_subscriber::layer::SubscriberExt;

    const TARGET: &str = "dedicated_target";

    #[derive(Default)]
    struct FakeSink {
        streams: Mutex<Vec<(String, String)>>,
        puts: Mutex<Vec<(String, String, Vec<LogRecord>)>>,
        failures_left: Mutex<u32>,
        refuse_streams: bool,
    }

    #[async_trait::async_trait]
    impl LogEventSink for Arc<FakeSink> {
        async fn create_stream(&self, group: &str, stream: &str) -> Result<(), BoxError> {
            if self.refuse_streams {
                return Err("synthetic access denied".into());
            }
            self.streams
                .lock()
                .unwrap()
                .push((group.to_owned(), stream.to_owned()));
            Ok(())
        }

        async fn put(
            &self,
            group: &str,
            stream: &str,
            records: &[LogRecord],
        ) -> Result<(), BoxError> {
            let mut failures_left = self.failures_left.lock().unwrap();
            if *failures_left > 0 {
                *failures_left -= 1;
                return Err("synthetic delivery failure".into());
            }
            self.puts
                .lock()
                .unwrap()
                .push((group.to_owned(), stream.to_owned(), records.to_vec()));
            Ok(())
        }
    }

    impl FakeSink {
        fn messages(&self, group: &str) -> Vec<serde_json::Value> {
            self.puts
                .lock()
                .unwrap()
                .iter()
                .filter(|(put_group, _, _)| put_group == group)
                .flat_map(|(_, _, records)| records.clone())
                .map(|record| serde_json::from_str(&record.message).unwrap())
                .collect()
        }
    }

    fn record(message: serde_json::Value, timestamp_ms: i64) -> LogRecord {
        LogRecord {
            timestamp_ms,
            message: message.to_string(),
        }
    }

    /// Records whose `fields.kind` is "second" go to the second group.
    fn route(line: &serde_json::Value) -> usize {
        usize::from(line["fields"]["kind"] == "second")
    }

    fn config() -> ShipperConfig {
        ShipperConfig {
            log_groups: vec!["/first".to_owned(), "/second".to_owned()],
            stream: "host/api/stream".to_owned(),
            route,
            flush_interval: Duration::from_secs(3600),
        }
    }

    #[test]
    fn should_queue_only_the_target_events_as_json_lines_whatever_else_is_filtered() {
        let (records, mut receiver) = channel(TARGET, 16);
        let subscriber = tracing_subscriber::registry().with(records.layer());

        tracing::subscriber::with_default(subscriber, || {
            let _span = tracing::info_span!("request", request_id = "r-1").entered();
            tracing::info!(target: TARGET, kind = "first", "Kept");
            tracing::info!(target: "other_target", "Not this target");
            tracing::debug!(target: TARGET, "Below INFO");
        });

        let queued = receiver.records.try_recv().unwrap();
        assert!(receiver.records.try_recv().is_err());
        let line: serde_json::Value = serde_json::from_str(&queued.message).unwrap();
        assert_eq!("INFO", line["level"]);
        assert_eq!(TARGET, line["target"]);
        assert_eq!("first", line["fields"]["kind"]);
        assert_eq!("Kept", line["fields"]["message"]);
        assert!(line.get("span").is_none() && line.get("timestamp").is_none());
        assert!(queued.timestamp_ms > 0);
    }

    #[test]
    fn should_drop_and_count_records_when_the_queue_is_full() {
        let (records, receiver) = channel(TARGET, 1);
        let subscriber = tracing_subscriber::registry().with(records.layer());

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: TARGET, "Queued");
            tracing::info!(target: TARGET, "Dropped");
        });

        assert_eq!(1, receiver.dropped.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn should_route_each_record_to_its_group_in_timestamp_order_on_close() {
        let (records, receiver) = channel(TARGET, 16);
        let sink = Arc::new(FakeSink::default());
        let shipper = Shipper::start(Arc::clone(&sink), receiver, config())
            .await
            .unwrap();
        for (kind, timestamp) in [("second", 3), ("first", 2), ("second", 1)] {
            records
                .writer
                .sender
                .send(record(
                    serde_json::json!({ "fields": { "kind": kind, "at": timestamp } }),
                    timestamp,
                ))
                .await
                .unwrap();
        }

        shipper.close(Duration::from_secs(5)).await;

        assert_eq!(
            vec![serde_json::json!({ "fields": { "kind": "first", "at": 2 } })],
            sink.messages("/first")
        );
        let second = sink.messages("/second");
        assert_eq!(
            vec![1, 3],
            second
                .iter()
                .map(|line| line["fields"]["at"].as_i64().unwrap())
                .collect::<Vec<_>>()
        );
        let mut streams = sink.streams.lock().unwrap().clone();
        streams.sort();
        assert_eq!(
            vec![
                ("/first".to_owned(), "host/api/stream".to_owned()),
                ("/second".to_owned(), "host/api/stream".to_owned()),
            ],
            streams
        );
    }

    #[tokio::test]
    async fn should_refuse_to_start_when_a_stream_cannot_be_created() {
        let (_, receiver) = channel(TARGET, 16);
        let sink = Arc::new(FakeSink {
            refuse_streams: true,
            ..FakeSink::default()
        });

        assert!(Shipper::start(sink, receiver, config()).await.is_err());
    }

    #[tokio::test]
    async fn should_retry_a_refused_delivery_and_deliver_each_record_once() {
        let (records, receiver) = channel(TARGET, 16);
        let sink = Arc::new(FakeSink {
            failures_left: Mutex::new(2),
            ..FakeSink::default()
        });
        let shipper = Shipper::start(
            Arc::clone(&sink),
            receiver,
            ShipperConfig {
                flush_interval: Duration::from_millis(10),
                ..config()
            },
        )
        .await
        .unwrap();
        records
            .writer
            .sender
            .send(record(
                serde_json::json!({ "fields": { "kind": "first" } }),
                1,
            ))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;
        shipper.close(Duration::from_secs(5)).await;

        assert_eq!(1, sink.messages("/first").len());
    }

    #[tokio::test(start_paused = true)]
    async fn should_keep_retrying_until_deliveries_have_failed_for_too_long() {
        let mut destination = Destination::new("/first".to_owned());
        destination.pending.push(record(serde_json::json!({}), 1));
        let sink = Arc::new(FakeSink {
            failures_left: Mutex::new(u32::MAX),
            ..FakeSink::default()
        });

        deliver(&sink, "stream", &mut destination).await;
        tokio::time::advance(GIVE_UP_AFTER - Duration::from_secs(1)).await;
        deliver(&sink, "stream", &mut destination).await;
        assert_eq!(1, destination.pending.len());

        tokio::time::advance(Duration::from_secs(1)).await;
        deliver(&sink, "stream", &mut destination).await;
        assert!(destination.pending.is_empty(), "spilled to stderr");

        *sink.failures_left.lock().unwrap() = 0;
        destination
            .pending
            .push(record(serde_json::json!({ "later": true }), 2));
        deliver(&sink, "stream", &mut destination).await;
        assert_eq!(
            vec![serde_json::json!({ "later": true })],
            sink.messages("/first")
        );
    }

    #[test]
    fn should_split_batches_within_the_put_log_events_limits() {
        let small = vec![record(serde_json::json!({}), 1); MAX_BATCH_EVENTS + 5];
        assert_eq!(MAX_BATCH_EVENTS, batch_len(&small));
        let large = vec![
            LogRecord {
                timestamp_ms: 1,
                message: "x".repeat(400_000),
            };
            3
        ];
        assert_eq!(2, batch_len(&large));
    }
}
