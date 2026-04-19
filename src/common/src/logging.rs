use tracing::Level;
use tracing_subscriber::EnvFilter;

fn parse_log_level(log_level: &str) -> Option<Level> {
    match log_level.to_ascii_uppercase().as_str() {
        "TRACE" => Some(Level::TRACE),
        "DEBUG" => Some(Level::DEBUG),
        "INFO" => Some(Level::INFO),
        "WARN" => Some(Level::WARN),
        "ERROR" => Some(Level::ERROR),
        _ => None,
    }
}

fn resolve_log_level(log_level: Option<&str>) -> Level {
    log_level
        .and_then(parse_log_level)
        .unwrap_or(tracing::Level::INFO)
}

pub fn init_logging() {
    let configured_log_level = std::env::var("LOG_LEVEL").ok();
    let log_level = resolve_log_level(configured_log_level.as_deref());

    tracing_subscriber::fmt()
        .json()
        .with_max_level(log_level)
        .with_current_span(true)
        .with_ansi(false)
        .without_time()
        .init();

    tracing::debug!(log_level = ?log_level, "Logger initialized.");
}

/// Like [`init_logging`] but accepts additional [`EnvFilter`] directives that
/// are appended after the global log level.
///
/// # Example
///
/// ```rust
/// // Suppress noisy third-party crates while keeping your own logs at INFO.
/// common::logging::init_logging_with_directives(&["spider=warn", "sqlx::postgres::notice=warn"]);
/// ```
pub fn init_logging_with_directives(extra_directives: &[&str]) {
    let configured_log_level = std::env::var("LOG_LEVEL").ok();
    let raw_level = configured_log_level
        .as_deref()
        .unwrap_or("info")
        .to_string();

    let directives = if extra_directives.is_empty() {
        raw_level
    } else {
        format!("{},{}", raw_level, extra_directives.join(","))
    };

    let filter = EnvFilter::new(directives);

    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
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
    fn should_use_info_when_no_log_level_for_default_logging() {
        assert_eq!(resolve_log_level(None), Level::INFO);
    }

    #[test]
    fn should_use_debug_when_debug_log_level_for_logging() {
        assert_eq!(resolve_log_level(Some("DEBUG")), Level::DEBUG);
    }

    #[test]
    fn should_use_info_when_invalid_log_level_for_logging() {
        assert_eq!(resolve_log_level(Some("INVALID")), Level::INFO);
    }
}
