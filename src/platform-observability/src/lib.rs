use tracing::{Event, Level, Subscriber};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    #[default]
    Warn,
    Error,
}

impl LogLevel {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_uppercase().as_str() {
            "TRACE" => Some(Self::Trace),
            "DEBUG" => Some(Self::Debug),
            "INFO" => Some(Self::Info),
            "WARN" => Some(Self::Warn),
            "ERROR" => Some(Self::Error),
            _ => None,
        }
    }

    const fn as_level(self) -> Level {
        match self {
            Self::Trace => Level::TRACE,
            Self::Debug => Level::DEBUG,
            Self::Info => Level::INFO,
            Self::Warn => Level::WARN,
            Self::Error => Level::ERROR,
        }
    }

    /// The finest of this level and INFO: the level spans are created at.
    const fn span_floor(self) -> Self {
        match self {
            Self::Warn | Self::Error => Self::Info,
            other => other,
        }
    }

    /// Whether events are filtered coarser than the spans around them.
    const fn filters_above_spans(self) -> bool {
        matches!(self, Self::Warn | Self::Error)
    }

    const fn as_directive(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LoggingConfig {
    level: LogLevel,
}

impl LoggingConfig {
    pub const fn new(level: LogLevel) -> Self {
        Self { level }
    }

    pub const fn level(self) -> LogLevel {
        self.level
    }

    /// The level named by `LOG_LEVEL`, or the default when it is unset or not a level.
    pub fn from_env() -> Self {
        let level = std::env::var("LOG_LEVEL")
            .ok()
            .as_deref()
            .and_then(LogLevel::parse)
            .unwrap_or_default();
        Self::new(level)
    }
}

pub fn init(config: LoggingConfig) {
    init_with_directives(config, &[]);
}

/// Initializes the JSON log at `config`'s level plus `extra_directives`, such as `rmcp=off` or
/// `some_target=info`, which can raise or silence one target.
pub fn init_with_directives(config: LoggingConfig, extra_directives: &[&str]) {
    subscriber(config, extra_directives, std::io::stdout).init();

    tracing::debug!(log_level = ?config.level(), "Logger initialized.");
}

/// Spans are created at INFO or finer whatever the level: the request and correlation IDs of the
/// request span are what ties a WARN or ERROR line to its request, and a span below the filter
/// is never created, so it would vanish from the line's `span` field. A level above INFO then
/// filters the events instead ([`EventLevelGate`]).
fn subscriber<W>(
    config: LoggingConfig,
    extra_directives: &[&str],
    writer: W,
) -> impl Subscriber + Send + Sync + 'static
where
    W: for<'writer> MakeWriter<'writer> + Send + Sync + 'static,
{
    let level = config.level();
    let with_extras = |base: &str| {
        std::iter::once(base)
            .chain(extra_directives.iter().copied())
            .collect::<Vec<_>>()
            .join(",")
    };
    let gate = level.filters_above_spans().then(|| {
        let events = with_extras(level.as_directive());
        EventLevelGate(
            events
                .parse::<Targets>()
                .unwrap_or_else(|_| Targets::new().with_default(level.as_level())),
        )
    });
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::new(with_extras(
            level.span_floor().as_directive(),
        )))
        .with_current_span(true)
        .with_ansi(false)
        .without_time()
        .with_writer(writer)
        .finish()
        .with(gate)
}

/// Drops the events the configured level excludes after their spans were created. It only gates
/// events: spans, and so the context of every event that passes, are left to the base filter.
struct EventLevelGate(Targets);

impl<S: Subscriber> Layer<S> for EventLevelGate {
    fn event_enabled(&self, event: &Event<'_>, _ctx: Context<'_, S>) -> bool {
        let metadata = event.metadata();
        self.0.would_enable(metadata.target(), metadata.level())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_parse_known_log_levels_case_insensitively() {
        assert_eq!(Some(LogLevel::Debug), LogLevel::parse("debug"));
    }

    #[test]
    fn should_reject_unknown_log_levels() {
        assert_eq!(None, LogLevel::parse("verbose"));
    }

    #[test]
    fn should_default_to_warn_so_production_logs_only_what_needs_attention() {
        assert_eq!(LogLevel::Warn, LoggingConfig::default().level());
    }

    #[derive(Clone, Default)]
    struct SharedBuffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for SharedBuffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// The JSON log lines `emit` writes under `level` and `extra_directives`.
    fn logged(level: LogLevel, extra_directives: &[&str], emit: impl FnOnce()) -> Vec<String> {
        let buffer = SharedBuffer::default();
        let writer = buffer.clone();
        let subscriber = subscriber(LoggingConfig::new(level), extra_directives, move || {
            writer.clone()
        });
        tracing::subscriber::with_default(subscriber, emit);
        let output = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
        output.lines().map(str::to_owned).collect()
    }

    /// The request span is INFO, so a filter that disabled it would strip the request and
    /// correlation IDs from the failure lines it is meant to keep.
    #[test]
    fn should_keep_the_request_span_on_a_warning_when_the_level_is_warn() {
        let lines = logged(LogLevel::Warn, &[], || {
            let _request = tracing::info_span!("http.request", request_id = "req-123").entered();
            tracing::info!("Routine line the quiet level drops");
            tracing::warn!("Request failed");
        });

        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains(r#""level":"WARN""#), "{}", lines[0]);
        assert!(
            lines[0].contains(r#""request_id":"req-123""#),
            "{}",
            lines[0]
        );
    }

    #[test]
    fn should_keep_a_directive_target_at_info_when_the_level_is_warn() {
        let lines = logged(
            LogLevel::Warn,
            &["kept_target=info", "dropped_target=off"],
            || {
                tracing::info!(target: "kept_target", "Kept");
                tracing::info!(target: "other_target", "Dropped by the level");
                tracing::error!(target: "dropped_target", "Dropped by its directive");
                tracing::error!(target: "other_target", "Reported");
            },
        );

        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains(r#""message":"Kept""#), "{}", lines[0]);
        assert!(lines[1].contains(r#""message":"Reported""#), "{}", lines[1]);
    }

    #[test]
    fn should_report_only_errors_when_the_level_is_error() {
        let lines = logged(LogLevel::Error, &[], || {
            tracing::warn!("Warning");
            tracing::error!("Failure");
        });

        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains(r#""level":"ERROR""#), "{}", lines[0]);
    }

    #[test]
    fn should_log_info_but_not_debug_when_the_level_is_info() {
        let lines = logged(LogLevel::Info, &[], || {
            tracing::debug!("Detail");
            tracing::info!("Routine");
        });

        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains(r#""level":"INFO""#), "{}", lines[0]);
    }
}
