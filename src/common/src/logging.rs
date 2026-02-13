use tracing::Level;

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
