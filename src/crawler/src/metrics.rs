//! Crawler metrics as CloudWatch Embedded Metric Format (EMF) records.
//!
//! A record is one JSON object whose top-level `_aws` block tells CloudWatch Logs which
//! keys of the object are metrics and which are dimensions, so the crawler publishes
//! metrics through the log export it already has, with no metrics client or extra
//! permission. Records travel as `tracing` events of [`EMF_TARGET`]; the log formats wrap
//! their JSON format in [`EmfPassthroughFormat`], which writes such an event as the bare
//! record instead of nesting it in the event's `fields`.
//!
//! [`docs/crawler/operations.md`](../../../docs/crawler/operations.md#metrics-and-alarms)
//! owns the namespace, metric names, dimensions and alarms. Dimension values are bounded
//! identifiers and kinds, never URLs.

use listing_source_core::ListingSourceId;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CrawlerMetric {
    /// Scraper URLs whose scrape completed, changed or not, per domain batch.
    ScrapesSucceeded,
    /// Scraper URLs whose scrape failed, per domain batch.
    ScrapesFailed,
    /// Scraper URLs whose page waits on a pending product-schema review, per domain batch: no
    /// failure, so neither in `ScrapesFailed` nor in `UrlFailures`.
    ScrapesAwaitingSchemaReview,
    /// Raw observations raw capture persisted durably, per capture batch.
    ProductsCaptured,
    /// Scraper URL failures of one `failure_kind`, per domain batch.
    UrlFailures,
    /// Scraper domain circuits opened.
    DomainCircuitOpened,
    /// Product URLs a spider crawl classified with the domain's URL pattern.
    CrawlProductUrls,
    /// 1 when a spider crawl was cut short before its bounds, else 0.
    CrawlTruncated,
    /// One spider crawl, recorded with its persisted `crawl_outcome`.
    CrawlOutcome,
    ScraperPassDuration,
    SpiderPassDuration,
    /// URLs due for scraping when a scraper pass starts.
    ScraperDueUrls,
}

impl CrawlerMetric {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::ScrapesSucceeded => "ScrapesSucceeded",
            Self::ScrapesFailed => "ScrapesFailed",
            Self::ScrapesAwaitingSchemaReview => "ScrapesAwaitingSchemaReview",
            Self::ProductsCaptured => "ProductsCaptured",
            Self::UrlFailures => "UrlFailures",
            Self::DomainCircuitOpened => "DomainCircuitOpened",
            Self::CrawlProductUrls => "CrawlProductUrls",
            Self::CrawlTruncated => "CrawlTruncated",
            Self::CrawlOutcome => "CrawlOutcome",
            Self::ScraperPassDuration => "ScraperPassDuration",
            Self::SpiderPassDuration => "SpiderPassDuration",
            Self::ScraperDueUrls => "ScraperDueUrls",
        }
    }

    const fn unit(self) -> MetricUnit {
        match self {
            Self::ScraperPassDuration | Self::SpiderPassDuration => MetricUnit::Milliseconds,
            _ => MetricUnit::Count,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetricDimension {
    ListingSourceId,
    /// The configured ListingSource domain, never a page URL.
    Domain,
    /// A persisted URL error kind such as `HTTP_503` or `Timeout`.
    FailureKind,
    /// A persisted `last_crawl_outcome` value.
    CrawlOutcome,
}

impl MetricDimension {
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::ListingSourceId => "listing_source_id",
            Self::Domain => "domain",
            Self::FailureKind => "failure_kind",
            Self::CrawlOutcome => "crawl_outcome",
        }
    }
}

/// One EMF record: metric values that share one dimension set.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MetricRecord {
    dimensions: Vec<(MetricDimension, String)>,
    values: Vec<(CrawlerMetric, u64)>,
}

impl MetricRecord {
    /// A record without dimensions, for crawler-wide metrics.
    pub(crate) fn global() -> Self {
        Self {
            dimensions: Vec::new(),
            values: Vec::new(),
        }
    }

    pub(crate) fn for_listing_source_domain(
        listing_source_id: &ListingSourceId,
        domain: &str,
    ) -> Self {
        Self::global()
            .with_dimension(
                MetricDimension::ListingSourceId,
                listing_source_id.to_string(),
            )
            .with_dimension(MetricDimension::Domain, domain)
    }

    pub(crate) fn with_dimension(
        mut self,
        dimension: MetricDimension,
        value: impl Into<String>,
    ) -> Self {
        self.dimensions.push((dimension, value.into()));
        self
    }

    pub(crate) fn with_value(mut self, metric: CrawlerMetric, value: u64) -> Self {
        self.values.push((metric, value));
        self
    }

    pub(crate) fn emit(&self) {
        let record = self.to_emf(time::OffsetDateTime::now_utc()).to_string();
        tracing::info!(target: EMF_TARGET, emf = record.as_str(), "Crawler metrics");
    }

    fn to_emf(&self, timestamp: time::OffsetDateTime) -> Value {
        let dimension_keys: Vec<&str> = self
            .dimensions
            .iter()
            .map(|(dimension, _)| dimension.key())
            .collect();
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
                    "Dimensions": [dimension_keys],
                    "Metrics": metric_definitions,
                }],
            }),
        );
        for (dimension, value) in &self.dimensions {
            record.insert(dimension.key().to_owned(), Value::from(value.as_str()));
        }
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
    use super::{CrawlerMetric, MetricDimension, MetricRecord, emf_record_of};
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

        /// Values of `metric` in emission order, each with the record's dimensions.
        pub(crate) fn values(&self, metric: CrawlerMetric) -> Vec<(Vec<(String, String)>, u64)> {
            self.0
                .lock()
                .expect("metric capture lock")
                .iter()
                .filter_map(|record| {
                    let value = record.get(metric.name())?.as_u64()?;
                    Some((dimensions_of(record), value))
                })
                .collect()
        }

        /// Values of `metric` whose record has `dimension` set to `expected`.
        pub(crate) fn values_with(
            &self,
            metric: CrawlerMetric,
            dimension: MetricDimension,
            expected: &str,
        ) -> Vec<u64> {
            self.values(metric)
                .into_iter()
                .filter(|(dimensions, _)| {
                    dimensions
                        .iter()
                        .any(|(key, value)| key == dimension.key() && value == expected)
                })
                .map(|(_, value)| value)
                .collect()
        }
    }

    fn dimensions_of(record: &Value) -> Vec<(String, String)> {
        record["_aws"]["CloudWatchMetrics"][0]["Dimensions"][0]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|key| {
                (
                    key.to_owned(),
                    record[key].as_str().unwrap_or_default().to_owned(),
                )
            })
            .collect()
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
            MetricRecord::global().emit();
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

    fn source_record() -> MetricRecord {
        MetricRecord::for_listing_source_domain(&ListingSourceId::new(), "shop.example")
            .with_value(CrawlerMetric::ScrapesSucceeded, 3)
            .with_value(CrawlerMetric::ScrapesFailed, 1)
    }

    #[test]
    fn should_describe_metrics_and_dimensions_in_a_top_level_aws_block() {
        let timestamp =
            time::OffsetDateTime::from_unix_timestamp(1_793_016_000).expect("valid test timestamp");
        let record = source_record().to_emf(timestamp);

        let directive = &record["_aws"]["CloudWatchMetrics"][0];
        assert_eq!(record["_aws"]["Timestamp"], 1_793_016_000_000_i64);
        assert_eq!(directive["Namespace"], "AuraHistoria/Crawler");
        assert_eq!(
            directive["Dimensions"],
            json!([["listing_source_id", "domain"]])
        );
        assert_eq!(
            directive["Metrics"],
            json!([
                { "Name": "ScrapesSucceeded", "Unit": "Count" },
                { "Name": "ScrapesFailed", "Unit": "Count" },
            ])
        );
        assert_eq!(record["domain"], "shop.example");
        assert!(record["listing_source_id"].is_string());
        assert_eq!(record["ScrapesSucceeded"], 3);
        assert_eq!(record["ScrapesFailed"], 1);
    }

    #[test]
    fn should_publish_global_metrics_under_an_empty_dimension_set() {
        let record = MetricRecord::global()
            .with_value(CrawlerMetric::ScraperPassDuration, 1_500)
            .to_emf(time::OffsetDateTime::now_utc());

        let directive = &record["_aws"]["CloudWatchMetrics"][0];
        assert_eq!(directive["Dimensions"], json!([[]]));
        assert_eq!(
            directive["Metrics"],
            json!([{ "Name": "ScraperPassDuration", "Unit": "Milliseconds" }])
        );
        assert_eq!(record["ScraperPassDuration"], 1_500);
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
                    .with_span_list(true),
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
            let _span = tracing::info_span!("scrape_domain", domain_id = "cd_test").entered();
            source_record().emit();
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
    fn should_keep_the_json_format_for_other_events() {
        let lines = json_log_lines(|| tracing::info!(outcome = "ok", "Scraper pass complete"));

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["fields"]["outcome"], "ok");
        assert!(lines[0].get("_aws").is_none());
    }
}
