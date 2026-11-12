//! Crawler metrics as CloudWatch Embedded Metric Format (EMF) records.
//!
//! A record is one JSON object whose top-level `_aws` block tells CloudWatch Logs which
//! keys of the object are metrics, so the crawler publishes metrics through the log export it
//! already has, with no metrics client or extra permission. Records travel as `tracing`
//! events of [`EMF_TARGET`]; the log formats wrap their JSON format in
//! [`EmfPassthroughFormat`], which writes such an event as the bare record instead of nesting
//! it in the event's `fields`.
//!
//! Every metric is crawler-wide and has no dimension: CloudWatch bills each distinct
//! dimension value as its own custom metric, so a per-source, per-domain or per-URL dimension
//! multiplies the bill by the number of sources, domains or URLs. A pass aggregates in process
//! and publishes one record when it ends. Per-source detail belongs in the outcome events of
//! the log, never in a metric. [`CrawlerMetric`] is the closed list of what is published.
//!
//! [`docs/crawler/operations.md`](../../../docs/crawler/operations.md#metrics-and-alarms)
//! owns the namespace, metric names and alarms.

use serde_json::{Map, Value, json};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::registry::LookupSpan;

pub const METRICS_NAMESPACE: &str = "AuraHistoria/Crawler";
/// The `tracing` target of metric records.
pub const EMF_TARGET: &str = "crawler::metrics";
/// Keeps metric records flowing whatever log level the operator configures, so a quiet
/// log level cannot read as a metric gap and trip the missing-data alarms.
pub const EMF_LOG_DIRECTIVE: &str = "crawler::metrics=info";
const EMF_FIELD: &str = "emf";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetricUnit {
    Count,
    Milliseconds,
}

impl MetricUnit {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Count => "Count",
            Self::Milliseconds => "Milliseconds",
        }
    }
}

/// Defines [`CrawlerMetric`] from one list, so its names, units and the test-only list of every
/// metric cannot drift apart.
macro_rules! crawler_metrics {
    ($($(#[$doc:meta])* $variant:ident => $unit:ident),+ $(,)?) => {
        /// The metrics the crawler publishes. Adding one is a cost decision, because CloudWatch
        /// bills each as a custom metric and the account's free tier covers ten, shared with the
        /// ingestion Lambda's metric filters: keep the list short and never give one a dimension.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) enum CrawlerMetric {
            $($(#[$doc])* $variant),+
        }

        impl CrawlerMetric {
            #[cfg(test)]
            pub(crate) const ALL: &'static [Self] = &[$(Self::$variant),+];

            pub(crate) const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => stringify!($variant)),+
                }
            }

            const fn unit(self) -> MetricUnit {
                match self {
                    $(Self::$variant => MetricUnit::$unit),+
                }
            }
        }
    };
}

crawler_metrics! {
    /// Raw observations the ingestion queue accepted, per scraper pass.
    ProductsCaptured => Count,
    /// Scraper URLs whose scrape produced a capture, per scraper pass. A page that matches its
    /// last capture is skipped, not scraped, and counts here no more than a failure.
    ScrapesSucceeded => Count,
    /// Scraper URLs whose scrape failed, per scraper pass.
    ScrapesFailed => Count,
    /// Scraper URL failures that recorded a failure kind, per scraper pass. A page that waits
    /// on a pending product-schema review is no failure and counts in neither this nor
    /// `ScrapesFailed`.
    UrlFailures => Count,
    /// Spider runs that ended in a failure, including one that stopped its ListingSource for an
    /// operator, per spider pass.
    CrawlRunsFailed => Count,
    ScraperPassDuration => Milliseconds,
    SpiderPassDuration => Milliseconds,
    /// URLs due for scraping when a scraper pass starts.
    ScraperDueUrls => Count,
}

/// One EMF record: the metric values of one pass, under the empty dimension set.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct MetricRecord {
    values: Vec<(CrawlerMetric, u64)>,
}

impl MetricRecord {
    pub(crate) fn with_value(mut self, metric: CrawlerMetric, value: u64) -> Self {
        self.values.push((metric, value));
        self
    }

    pub(crate) fn emit(&self) {
        let record = self.to_emf(time::OffsetDateTime::now_utc()).to_string();
        tracing::info!(target: EMF_TARGET, emf = record.as_str(), "Crawler metrics");
    }

    fn to_emf(&self, timestamp: time::OffsetDateTime) -> Value {
        let metric_definitions: Vec<Value> = self
            .values
            .iter()
            .map(|(metric, _)| json!({ "Name": metric.name(), "Unit": metric.unit().as_str() }))
            .collect();
        let timestamp_ms = i64::try_from(timestamp.unix_timestamp_nanos() / 1_000_000)
            .expect("a current timestamp in milliseconds fits i64");

        let mut record = Map::new();
        record.insert(
            "_aws".to_owned(),
            json!({
                "Timestamp": timestamp_ms,
                "CloudWatchMetrics": [{
                    "Namespace": METRICS_NAMESPACE,
                    "Dimensions": [[]],
                    "Metrics": metric_definitions,
                }],
            }),
        );
        for (metric, value) in &self.values {
            record.insert(metric.name().to_owned(), Value::from(*value));
        }
        Value::Object(record)
    }
}

/// The EMF record an event carries, when it is a metric record.
fn emf_record_of(event: &Event<'_>) -> Option<String> {
    if event.metadata().target() != EMF_TARGET {
        return None;
    }
    let mut visitor = EmfFieldVisitor(None);
    event.record(&mut visitor);
    visitor.0
}

struct EmfFieldVisitor(Option<String>);

impl Visit for EmfFieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == EMF_FIELD {
            self.0 = Some(value.to_owned());
        }
    }

    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
}

/// Writes a metric record event as the bare EMF record, so `_aws` stays a top-level key
/// of the log line as CloudWatch requires, and every other event with `inner`.
pub struct EmfPassthroughFormat<F> {
    inner: F,
}

impl<F> EmfPassthroughFormat<F> {
    pub fn new(inner: F) -> Self {
        Self { inner }
    }
}

impl<S, N, F> FormatEvent<S, N> for EmfPassthroughFormat<F>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
    F: FormatEvent<S, N>,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result {
        match emf_record_of(event) {
            Some(record) => writeln!(writer, "{record}"),
            None => self.inner.format_event(ctx, writer, event),
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::{MetricRecord, emf_record_of};
    use serde_json::Value;
    use std::sync::{Arc, Mutex};
    use tracing::Subscriber;
    use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

    /// Collects the metric records emitted on this thread while its guard lives.
    #[derive(Clone, Default)]
    pub(crate) struct CapturedMetrics(Arc<Mutex<Vec<Value>>>);

    impl<S: Subscriber> Layer<S> for CapturedMetrics {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            if let Some(record) = emf_record_of(event) {
                let record = serde_json::from_str(&record).expect("EMF record must be JSON");
                self.0.lock().expect("metric capture lock").push(record);
            }
        }
    }

    impl CapturedMetrics {
        /// Takes every record captured so far.
        fn drain(&self) -> Vec<Value> {
            std::mem::take(&mut *self.0.lock().expect("metric capture lock"))
        }

        /// Every record captured so far, as the log line carries it.
        pub(crate) fn records(&self) -> Vec<Value> {
            self.0.lock().expect("metric capture lock").clone()
        }
    }

    pub(crate) fn capture_metrics() -> (CapturedMetrics, tracing::subscriber::DefaultGuard) {
        let captured = CapturedMetrics::default();
        let subscriber = tracing_subscriber::registry().with(captured.clone());
        let guard = tracing::subscriber::set_default(subscriber);
        prime_metric_callsite(|| captured.drain().len() == 1);
        (captured, guard)
    }

    /// Makes the metric callsite reach this thread's default subscriber. `tracing` caches
    /// the interest in a callsite once for the whole process, decided by the thread that
    /// first uses it: a test thread without a subscriber, such as one driving a scheduler
    /// pass without capturing, caches "never", and a thread-local subscriber set later
    /// never sees a record. This emits a probe record, which `probe_observed` must consume
    /// and report, and rebuilds the cache when another thread won that race.
    pub(crate) fn prime_metric_callsite(mut probe_observed: impl FnMut() -> bool) {
        for _ in 0..2 {
            MetricRecord::default().emit();
            if probe_observed() {
                return;
            }
            tracing::callsite::rebuild_interest_cache();
        }
        panic!("the metric callsite stays disabled for this thread's subscriber");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::layer::SubscriberExt;

    fn scrape_record() -> MetricRecord {
        MetricRecord::default()
            .with_value(CrawlerMetric::ScrapesSucceeded, 3)
            .with_value(CrawlerMetric::ScrapesFailed, 1)
    }

    #[test]
    fn should_describe_metrics_in_a_top_level_aws_block_without_dimensions() {
        let timestamp =
            time::OffsetDateTime::from_unix_timestamp(1_793_016_000).expect("valid test timestamp");
        let record = scrape_record().to_emf(timestamp);

        let directive = &record["_aws"]["CloudWatchMetrics"][0];
        assert_eq!(record["_aws"]["Timestamp"], 1_793_016_000_000_i64);
        assert_eq!(directive["Namespace"], "AuraHistoria/Crawler");
        assert_eq!(directive["Dimensions"], json!([[]]));
        assert_eq!(
            directive["Metrics"],
            json!([
                { "Name": "ScrapesSucceeded", "Unit": "Count" },
                { "Name": "ScrapesFailed", "Unit": "Count" },
            ])
        );
        assert_eq!(record["ScrapesSucceeded"], 3);
        assert_eq!(record["ScrapesFailed"], 1);
    }

    #[test]
    fn should_publish_durations_in_milliseconds() {
        let record = MetricRecord::default()
            .with_value(CrawlerMetric::ScraperPassDuration, 1_500)
            .to_emf(time::OffsetDateTime::now_utc());

        assert_eq!(
            record["_aws"]["CloudWatchMetrics"][0]["Metrics"],
            json!([{ "Name": "ScraperPassDuration", "Unit": "Milliseconds" }])
        );
        assert_eq!(record["ScraperPassDuration"], 1_500);
    }

    /// Each distinct metric and dimension value is billed as a custom metric: the crawler may use
    /// eight of the account's ten free ones, and no dimension.
    #[test]
    fn should_publish_at_most_eight_metrics_and_no_dimension() {
        assert!(CrawlerMetric::ALL.len() <= 8, "{:?}", CrawlerMetric::ALL);
        let record = CrawlerMetric::ALL
            .iter()
            .fold(MetricRecord::default(), |record, metric| {
                record.with_value(*metric, 1)
            })
            .to_emf(time::OffsetDateTime::now_utc());

        let directive = &record["_aws"]["CloudWatchMetrics"][0];
        let names: std::collections::BTreeSet<&str> = directive["Metrics"]
            .as_array()
            .expect("metric definitions")
            .iter()
            .map(|metric| metric["Name"].as_str().expect("metric name"))
            .collect();
        assert_eq!(directive["Dimensions"], json!([[]]));
        assert_eq!(
            names,
            std::collections::BTreeSet::from([
                "CrawlRunsFailed",
                "ProductsCaptured",
                "ScraperDueUrls",
                "ScraperPassDuration",
                "ScrapesFailed",
                "ScrapesSucceeded",
                "SpiderPassDuration",
                "UrlFailures",
            ])
        );
        let top_level_keys: Vec<&String> = record.as_object().expect("record").keys().collect();
        assert_eq!(
            top_level_keys.len(),
            1 + names.len(),
            "the record holds the _aws block and the metric values only, so no dimension value"
        );
    }

    #[derive(Clone, Default)]
    struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedBuffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("buffer lock").extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn json_log_lines(log: impl FnOnce()) -> Vec<Value> {
        let buffer = SharedBuffer::default();
        let writer = buffer.clone();
        let layer = tracing_subscriber::fmt::layer()
            .json()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .event_format(EmfPassthroughFormat::new(
                tracing_subscriber::fmt::format()
                    .json()
                    .with_current_span(true)
                    .with_span_list(false),
            ));
        let probe_buffer = buffer.clone();
        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
            test_support::prime_metric_callsite(|| {
                let mut probe = probe_buffer.0.lock().expect("buffer lock");
                let observed = !probe.is_empty();
                probe.clear();
                observed
            });
            log();
        });
        let output = String::from_utf8(buffer.0.lock().expect("buffer lock").clone())
            .expect("log output is UTF-8");
        output
            .lines()
            .map(|line| serde_json::from_str(line).expect("each log line is JSON"))
            .collect()
    }

    #[test]
    fn should_write_a_metric_event_as_the_bare_record_inside_a_span() {
        let lines = json_log_lines(|| {
            let _span =
                tracing::info_span!("scrape_batch", listing_source_id = "ls_test").entered();
            scrape_record().emit();
        });

        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        assert_eq!(
            line["_aws"]["CloudWatchMetrics"][0]["Namespace"],
            "AuraHistoria/Crawler"
        );
        assert_eq!(line["ScrapesSucceeded"], 3);
        assert!(line.get("fields").is_none());
        assert!(line.get("span").is_none());
    }

    #[test]
    fn should_write_the_current_span_without_the_span_list_for_other_events() {
        let lines = json_log_lines(|| {
            let _outer = tracing::info_span!("outer").entered();
            let _inner = tracing::info_span!("inner").entered();
            tracing::info!("Inside spans");
        });

        assert_eq!(lines[0]["span"]["name"], "inner");
        assert!(lines[0].get("spans").is_none());
    }

    #[test]
    fn should_keep_the_json_format_for_other_events() {
        let lines = json_log_lines(|| tracing::info!(outcome = "ok", "Scraper pass complete"));

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["fields"]["outcome"], "ok");
        assert!(lines[0].get("_aws").is_none());
    }
}
