use platform_postgres::{PostgresFailure, SqlState};
use std::error::Error;

/// The category recorded when no adapter-specific classification recognizes the cause.
pub const UNCLASSIFIED_FAILURE: &str = "unclassified";

/// Safe, bounded facts about a failure, carried to the Lambda boundary that records it once.
///
/// Stage, dependency and category are allowlisted static labels; the HTTP status and SQLSTATE are
/// standardized codes. Error messages and source chains stay with the error value because they
/// can contain provider payloads, persisted values, private input or credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailureDiagnostic {
    stage: &'static str,
    dependency: Option<&'static str>,
    category: &'static str,
    http_status: Option<u16>,
    sqlstate: Option<SqlState>,
}

impl FailureDiagnostic {
    /// Classifies a non-PostgreSQL dependency whose adapter supplied the category and status.
    pub const fn new(
        stage: &'static str,
        dependency: &'static str,
        category: &'static str,
        http_status: Option<u16>,
    ) -> Self {
        Self {
            stage,
            dependency: Some(dependency),
            category,
            http_status,
            sqlstate: None,
        }
    }

    /// Records a request the use case refused itself, so no dependency was involved.
    pub const fn rejected(stage: &'static str, category: &'static str) -> Self {
        Self {
            stage,
            dependency: None,
            category,
            http_status: None,
            sqlstate: None,
        }
    }

    /// Classifies a PostgreSQL stage from the first SQLx error in the chain.
    pub fn postgres(stage: &'static str, error: &(dyn Error + 'static)) -> Self {
        let failure = PostgresFailure::find(error);
        Self {
            stage,
            dependency: Some("postgres"),
            category: failure.map_or(UNCLASSIFIED_FAILURE, |failure| failure.kind()),
            http_status: None,
            sqlstate: failure.and_then(|failure| failure.sqlstate()),
        }
    }

    pub fn stage(&self) -> &'static str {
        self.stage
    }

    pub fn dependency(&self) -> Option<&'static str> {
        self.dependency
    }

    pub fn category(&self) -> &'static str {
        self.category
    }

    pub fn http_status(&self) -> Option<u16> {
        self.http_status
    }

    pub fn sqlstate(&self) -> Option<&str> {
        self.sqlstate.as_ref().map(SqlState::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("failed to begin transaction")]
    struct BeginFailed(#[source] sqlx::Error);

    #[test]
    fn should_classify_a_postgres_stage_from_the_sqlx_error_in_the_chain() {
        let error = BeginFailed(sqlx::Error::PoolTimedOut);

        let failure = FailureDiagnostic::postgres("source_begin", &error);

        assert_eq!("source_begin", failure.stage());
        assert_eq!(Some("postgres"), failure.dependency());
        assert_eq!("pool_timed_out", failure.category());
        assert_eq!(None, failure.sqlstate());
    }

    #[test]
    fn should_mark_a_postgres_stage_unclassified_without_an_sqlx_error() {
        let error = std::io::Error::other("persisted value rejected");

        let failure = FailureDiagnostic::postgres("source_read", &error);

        assert_eq!(UNCLASSIFIED_FAILURE, failure.category());
    }

    #[test]
    fn should_record_a_refused_request_without_a_dependency() {
        let failure = FailureDiagnostic::rejected("resolution", "forbidden");

        assert_eq!(None, failure.dependency());
        assert_eq!("forbidden", failure.category());
    }
}
