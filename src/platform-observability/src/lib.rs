use tracing::Level;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    #[default]
    Info,
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
}

pub fn init(config: LoggingConfig) {
    tracing_subscriber::fmt()
        .json()
        .with_max_level(config.level().as_level())
        .with_current_span(true)
        .with_ansi(false)
        .without_time()
        .init();

    tracing::debug!(log_level = ?config.level(), "Logger initialized.");
}

pub fn init_with_directives(config: LoggingConfig, extra_directives: &[&str]) {
    let directives = if extra_directives.is_empty() {
        config.level().as_directive().to_owned()
    } else {
        format!(
            "{},{}",
            config.level().as_directive(),
            extra_directives.join(",")
        )
    };

    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::new(directives))
        .with_current_span(true)
        .with_ansi(false)
        .without_time()
        .init();

    tracing::debug!("Logger initialized with extra directives.");
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
    fn should_default_to_info() {
        assert_eq!(LogLevel::Info, LoggingConfig::default().level());
    }
}
